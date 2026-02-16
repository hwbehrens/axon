use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::warn;

use super::replay_cache::ReplayCache;
use crate::ipc::{CommandEvent, IpcBackend, IpcServer, PeerSummary, SendResult, StatusResult};
use crate::message::{AgentId, Envelope};
use crate::peer_table::{ConnectionStatus, PeerSource, PeerTable};
use crate::transport::QuicTransport;

#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) sent: AtomicU64,
    pub(crate) received: AtomicU64,
}

#[derive(Debug)]
pub(crate) enum DaemonIpcError {
    PeerNotFound,
    PeerUnreachable,
    InvalidCommand(String),
}

impl std::fmt::Display for DaemonIpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonIpcError::PeerNotFound => write!(f, "peer_not_found"),
            DaemonIpcError::PeerUnreachable => write!(f, "peer_unreachable"),
            DaemonIpcError::InvalidCommand(msg) => write!(f, "invalid_command: {msg}"),
        }
    }
}

impl std::error::Error for DaemonIpcError {}

pub(crate) struct DaemonContext<'a> {
    pub(crate) ipc: &'a IpcServer,
    pub(crate) peer_table: &'a PeerTable,
    pub(crate) transport: &'a QuicTransport,
    pub(crate) local_agent_id: &'a AgentId,
    pub(crate) counters: &'a Counters,
    pub(crate) replay_cache: &'a ReplayCache,
    pub(crate) start: Instant,
}

pub(crate) fn status_str(status: &ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Discovered => "discovered",
        ConnectionStatus::Connecting => "connecting",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::Disconnected => "disconnected",
    }
}

pub(crate) fn source_str(source: &PeerSource) -> &'static str {
    match source {
        PeerSource::Static => "static",
        PeerSource::Discovered => "discovered",
        PeerSource::Cached => "cached",
    }
}

// ---------------------------------------------------------------------------
// DaemonIpcBackend — implements IpcBackend using daemon resources
// ---------------------------------------------------------------------------

pub(crate) struct DaemonIpcBackend<'a> {
    pub(crate) peer_table: &'a PeerTable,
    pub(crate) transport: &'a QuicTransport,
    pub(crate) local_agent_id: &'a AgentId,
    pub(crate) counters: &'a Counters,
    pub(crate) replay_cache: &'a ReplayCache,
    pub(crate) start: Instant,
}

impl IpcBackend for DaemonIpcBackend<'_> {
    async fn send_message(
        &self,
        to: String,
        kind: crate::message::MessageKind,
        payload: serde_json::Value,
        ref_id: Option<uuid::Uuid>,
    ) -> Result<SendResult> {
        let peer = self
            .peer_table
            .get(&to)
            .await
            .ok_or_else(|| anyhow::anyhow!(DaemonIpcError::PeerNotFound))?;

        // Initiator rule: lower agent_id initiates. If we are higher,
        // wait briefly for the peer to connect, then error if still absent.
        if self.local_agent_id.as_str() > to.as_str() && !self.transport.has_connection(&to).await {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if !self.transport.has_connection(&to).await {
                anyhow::bail!(DaemonIpcError::PeerUnreachable);
            }
        }

        let mut envelope = Envelope::new((*self.local_agent_id).clone(), to.clone(), kind, payload);
        envelope.ref_id = ref_id;

        envelope
            .validate()
            .map_err(|e| anyhow::anyhow!(DaemonIpcError::InvalidCommand(e.to_string())))?;

        let msg_id = envelope.id;

        match self.transport.send(&peer, envelope).await {
            Ok(response) => {
                self.counters.sent.fetch_add(1, Ordering::Relaxed);
                self.peer_table.set_connected(&to, None).await;

                let response = if let Some(response_envelope) = response {
                    if self
                        .replay_cache
                        .is_replay(response_envelope.id, Instant::now())
                        .await
                    {
                        warn!(msg_id = %response_envelope.id, "dropping replayed response");
                        None
                    } else {
                        self.counters.received.fetch_add(1, Ordering::Relaxed);
                        Some(response_envelope)
                    }
                } else {
                    None
                };

                Ok(SendResult { msg_id, response })
            }
            Err(_err) => {
                self.peer_table.set_disconnected(&to).await;
                anyhow::bail!(DaemonIpcError::PeerUnreachable)
            }
        }
    }

    async fn peers(&self) -> Result<Vec<PeerSummary>> {
        Ok(self
            .peer_table
            .list()
            .await
            .into_iter()
            .map(|p| PeerSummary {
                id: p.agent_id.to_string(),
                addr: p.addr.to_string(),
                status: status_str(&p.status).to_string(),
                rtt_ms: p.rtt_ms,
                source: source_str(&p.source).to_string(),
            })
            .collect())
    }

    async fn status(&self) -> Result<StatusResult> {
        let peers_connected = self
            .peer_table
            .list()
            .await
            .iter()
            .filter(|p| p.status == ConnectionStatus::Connected)
            .count();

        Ok(StatusResult {
            uptime_secs: self.start.elapsed().as_secs(),
            peers_connected,
            messages_sent: self.counters.sent.load(Ordering::Relaxed),
            messages_received: self.counters.received.load(Ordering::Relaxed),
        })
    }
}

// ---------------------------------------------------------------------------
// Command dispatch — routes all IPC commands through the IPC handler layer
// ---------------------------------------------------------------------------

pub(crate) async fn handle_command(cmd: CommandEvent, ctx: &DaemonContext<'_>) -> Result<()> {
    let backend = DaemonIpcBackend {
        peer_table: ctx.peer_table,
        transport: ctx.transport,
        local_agent_id: ctx.local_agent_id,
        counters: ctx.counters,
        replay_cache: ctx.replay_cache,
        start: ctx.start,
    };

    let result = ctx
        .ipc
        .dispatch_command(cmd.client_id, cmd.command, &backend)
        .await?;
    ctx.ipc.send_reply(cmd.client_id, &result.reply).await?;

    // If send produced a response envelope, broadcast it to IPC clients
    if let Some(envelope) = result.response_envelope {
        ctx.ipc.broadcast_inbound(&envelope).await?;
    }

    // If the handler requested closing this client, do so
    if result.close {
        ctx.ipc.close_client(cmd.client_id).await;
    }

    Ok(())
}
