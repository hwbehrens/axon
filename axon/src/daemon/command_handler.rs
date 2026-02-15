use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::warn;

use super::replay_cache::ReplayCache;
use crate::ipc::{CommandEvent, DaemonReply, IpcCommand, IpcServer, PeerSummary};
use crate::message::{AgentId, Envelope};
use crate::peer_table::{ConnectionStatus, PeerSource, PeerTable};
use crate::transport::QuicTransport;

#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) sent: AtomicU64,
    pub(crate) received: AtomicU64,
}

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

pub(crate) fn instructive_send_error(peer_id: &str, err: &anyhow::Error) -> String {
    let root_cause = err.root_cause();
    format!(
        "Failed to reach peer {peer_id}: {root_cause}. \
         Check that the peer's daemon is running and reachable."
    )
}

pub(crate) async fn handle_command(cmd: CommandEvent, ctx: &DaemonContext<'_>) -> Result<()> {
    let DaemonContext {
        ipc,
        peer_table,
        transport,
        local_agent_id,
        counters,
        replay_cache,
        ..
    } = ctx;
    match cmd.command {
        IpcCommand::Send {
            to,
            kind,
            payload,
            ref_id,
        } => {
            let Some(peer) = peer_table.get(&to).await else {
                ipc.send_reply(
                    cmd.client_id,
                    &DaemonReply::Error {
                        ok: false,
                        error: format!(
                            "Peer {to} not found. Run 'axon peers' to see known peers, \
                             or check that the peer's daemon is running."
                        ),
                    },
                )
                .await?;
                return Ok(());
            };

            // Initiator rule: lower agent_id initiates. If we are higher,
            // wait briefly for the peer to connect, then error if still absent.
            if local_agent_id.as_str() > to.as_str() && !transport.has_connection(&to).await {
                // Wait briefly for the remote peer to initiate
                tokio::time::sleep(Duration::from_secs(2)).await;
                if !transport.has_connection(&to).await {
                    ipc.send_reply(
                        cmd.client_id,
                        &DaemonReply::Error {
                            ok: false,
                            error: format!(
                                "Initiator rule: peer {to} has a lower agent_id and should \
                                 initiate the connection. No inbound connection received within 2s. \
                                 Check that the peer's daemon is running."
                            ),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            }

            let mut envelope = Envelope::new((*local_agent_id).clone(), to.clone(), kind, payload);
            envelope.ref_id = ref_id;

            if let Err(err) = envelope.validate() {
                ipc.send_reply(
                    cmd.client_id,
                    &DaemonReply::Error {
                        ok: false,
                        error: format!(
                            "Invalid envelope: {err}. Check the message payload and agent IDs."
                        ),
                    },
                )
                .await?;
                return Ok(());
            }

            let msg_id = envelope.id;

            match transport.send(&peer, envelope).await {
                Ok(response) => {
                    counters.sent.fetch_add(1, Ordering::Relaxed);
                    peer_table.set_connected(&to, None).await;
                    ipc.send_reply(cmd.client_id, &DaemonReply::SendAck { ok: true, msg_id })
                        .await?;

                    if let Some(response_envelope) = response {
                        if replay_cache
                            .is_replay(response_envelope.id, Instant::now())
                            .await
                        {
                            warn!(msg_id = %response_envelope.id, "dropping replayed response");
                        } else {
                            counters.received.fetch_add(1, Ordering::Relaxed);
                            ipc.broadcast_inbound(&response_envelope).await?;
                        }
                    }
                }
                Err(err) => {
                    peer_table.set_disconnected(&to).await;
                    ipc.send_reply(
                        cmd.client_id,
                        &DaemonReply::Error {
                            ok: false,
                            error: instructive_send_error(&to, &err),
                        },
                    )
                    .await?;
                }
            }
        }
        IpcCommand::Peers => {
            let peers = peer_table
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
                .collect();

            ipc.send_reply(cmd.client_id, &DaemonReply::Peers { ok: true, peers })
                .await?;
        }
        IpcCommand::Status => {
            let peers_connected = peer_table
                .list()
                .await
                .iter()
                .filter(|p| p.status == ConnectionStatus::Connected)
                .count();

            ipc.send_reply(
                cmd.client_id,
                &DaemonReply::Status {
                    ok: true,
                    uptime_secs: ctx.start.elapsed().as_secs(),
                    peers_connected,
                    messages_sent: counters.sent.load(Ordering::Relaxed),
                    messages_received: counters.received.load(Ordering::Relaxed),
                },
            )
            .await?;
        }
    }

    Ok(())
}
