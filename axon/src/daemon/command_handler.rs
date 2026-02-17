use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ipc::{
    CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcSendKind, IpcServer, PeerSummary,
};
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
    SelfSend,
    PeerUnreachable,
    InvalidCommand(String),
}

impl std::fmt::Display for DaemonIpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonIpcError::PeerNotFound => write!(f, "peer_not_found"),
            DaemonIpcError::SelfSend => write!(f, "self_send"),
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
// Command dispatch — directly handles all IPC commands
// ---------------------------------------------------------------------------

pub(crate) async fn handle_command(cmd: CommandEvent, ctx: &DaemonContext<'_>) -> Result<()> {
    let client_id = cmd.client_id;

    let reply = match cmd.command {
        IpcCommand::Send {
            to,
            kind,
            payload,
            ref_id,
            req_id,
        } => match handle_send(ctx, to, kind, payload, ref_id).await {
            Ok((msg_id, response)) => {
                let broadcast_envelope = response.clone();
                let reply = DaemonReply::SendOk {
                    ok: true,
                    msg_id,
                    req_id,
                    response,
                };
                // Send reply first, then broadcast (so sender gets ack before broadcast)
                ctx.ipc.send_reply(client_id, &reply).await?;
                if let Some(envelope) = broadcast_envelope {
                    let _ = ctx.ipc.broadcast_inbound(&envelope).await;
                }
                return Ok(());
            }
            Err(e) => {
                let error_code = if let Some(e) = e.downcast_ref::<DaemonIpcError>() {
                    match e {
                        DaemonIpcError::PeerNotFound => IpcErrorCode::PeerNotFound,
                        DaemonIpcError::SelfSend => IpcErrorCode::SelfSend,
                        DaemonIpcError::PeerUnreachable => IpcErrorCode::PeerUnreachable,
                        DaemonIpcError::InvalidCommand(_) => IpcErrorCode::InvalidCommand,
                    }
                } else {
                    IpcErrorCode::InternalError
                };
                DaemonReply::Error {
                    ok: false,
                    message: error_code.message(),
                    error: error_code,
                    req_id,
                }
            }
        },
        IpcCommand::Peers { req_id } => {
            let peers: Vec<PeerSummary> = ctx
                .peer_table
                .list()
                .await
                .into_iter()
                .map(|p| PeerSummary {
                    agent_id: p.agent_id.to_string(),
                    addr: p.addr.to_string(),
                    status: status_str(&p.status).to_string(),
                    rtt_ms: p.rtt_ms,
                    source: source_str(&p.source).to_string(),
                })
                .collect();
            DaemonReply::Peers {
                ok: true,
                peers,
                req_id,
            }
        }
        IpcCommand::Status { req_id } => {
            let peers_connected = ctx
                .peer_table
                .list()
                .await
                .iter()
                .filter(|p| p.status == ConnectionStatus::Connected)
                .count();
            DaemonReply::Status {
                ok: true,
                uptime_secs: ctx.start.elapsed().as_secs(),
                peers_connected,
                messages_sent: ctx.counters.sent.load(Ordering::Relaxed),
                messages_received: ctx.counters.received.load(Ordering::Relaxed),
                req_id,
            }
        }
        IpcCommand::Whoami { req_id } => {
            // Forward to IPC server which has the config info
            ctx.ipc
                .handle_command(CommandEvent {
                    client_id,
                    command: IpcCommand::Whoami { req_id },
                })
                .await?
        }
    };

    ctx.ipc.send_reply(client_id, &reply).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Send helper
// ---------------------------------------------------------------------------

async fn handle_send(
    ctx: &DaemonContext<'_>,
    to: String,
    kind: IpcSendKind,
    payload: serde_json::Value,
    ref_id: Option<uuid::Uuid>,
) -> Result<(uuid::Uuid, Option<crate::message::Envelope>)> {
    if to == ctx.local_agent_id.as_str() {
        anyhow::bail!(DaemonIpcError::SelfSend);
    }

    let peer = ctx
        .peer_table
        .get(&to)
        .await
        .ok_or_else(|| anyhow::anyhow!(DaemonIpcError::PeerNotFound))?;

    let mut envelope = Envelope::new(
        (*ctx.local_agent_id).clone(),
        to.clone(),
        kind.as_message_kind(),
        payload,
    );
    envelope.ref_id = ref_id;
    envelope
        .validate()
        .map_err(|e| anyhow::anyhow!(DaemonIpcError::InvalidCommand(e.to_string())))?;

    let msg_id = envelope.id;

    // Timeout the send (including connection attempt) so IPC clients don't
    // block indefinitely when the peer is unreachable over UDP/QUIC.
    const SEND_TIMEOUT: Duration = Duration::from_secs(10);
    let send_result = tokio::time::timeout(SEND_TIMEOUT, ctx.transport.send(&peer, envelope)).await;

    match send_result {
        Err(_elapsed) => {
            ctx.peer_table.set_disconnected(&to).await;
            anyhow::bail!(DaemonIpcError::PeerUnreachable)
        }
        Ok(inner) => match inner {
            Ok(response) => {
                ctx.counters.sent.fetch_add(1, Ordering::Relaxed);
                ctx.peer_table.set_connected(&to, None).await;
                if response.is_some() {
                    ctx.counters.received.fetch_add(1, Ordering::Relaxed);
                }
                Ok((msg_id, response))
            }
            Err(_err) => {
                ctx.peer_table.set_disconnected(&to).await;
                anyhow::bail!(DaemonIpcError::PeerUnreachable)
            }
        },
    }
}
