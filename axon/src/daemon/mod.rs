pub(crate) mod command_handler;
mod peer_events;
mod reconnect;

use command_handler::{Counters, DaemonContext, handle_command};
use peer_events::handle_peer_event;
use reconnect::{ReconnectState, attempt_reconnects};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::{AxonPaths, Config, load_known_peers, save_known_peers};
use crate::discovery::{run_mdns_discovery, run_static_discovery};
use crate::identity::Identity;
use crate::ipc::IpcServer;
use crate::message::AgentId;
use crate::peer_table::PeerTable;
use crate::transport::QuicTransport;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct DaemonOptions {
    pub port: Option<u16>,
    pub disable_mdns: bool,
    pub axon_root: Option<PathBuf>,
    pub agent_id: Option<String>,
    pub cancel: Option<CancellationToken>,
}

// ---------------------------------------------------------------------------
// Daemon entry point
// ---------------------------------------------------------------------------

pub async fn run_daemon(opts: DaemonOptions) -> Result<()> {
    let paths = match opts.axon_root {
        Some(ref root) => AxonPaths::from_root(root.clone()),
        None => AxonPaths::discover()?,
    };
    paths.ensure_root_exists()?;

    let config = Config::load(&paths.config)?;
    let port = config.effective_port(opts.port);

    let identity = Identity::load_or_generate(&paths)?;
    let local_agent_id: AgentId = opts
        .agent_id
        .map(AgentId::from)
        .unwrap_or_else(|| AgentId::from(identity.agent_id()));

    info!(agent_id = %local_agent_id, port, "starting AXON daemon");

    // --- Clock validation ---
    let clock_ms = crate::message::now_millis();
    if clock_ms == 0 {
        anyhow::bail!(
            "system clock appears invalid (before UNIX epoch). \
             AXON requires a valid system clock for message timestamps. \
             Fix your system time (e.g., configure NTP) and try again."
        );
    }

    // --- Cancellation token for structured shutdown ---
    let cancel = opts.cancel.unwrap_or_default();

    // --- Peer table ---
    let peer_table = PeerTable::new();
    for peer in &config.peers {
        peer_table.upsert_static(peer).await;
    }
    for peer in load_known_peers(&paths.known_peers)? {
        peer_table.upsert_cached(&peer).await;
    }

    // --- Counters ---
    let counters = Arc::new(Counters::default());

    // --- Transport ---
    let bind_addr = format!("0.0.0.0:{port}")
        .parse()
        .context("invalid bind address")?;
    let transport = QuicTransport::bind_cancellable(
        bind_addr,
        &identity,
        cancel.clone(),
        config.effective_max_connections(),
        config.effective_keepalive(),
        config.effective_idle_timeout(),
        None,
        config.effective_inbound_read_timeout(),
        peer_table.pubkey_map(),
    )
    .await?;

    // --- IPC ---
    let start = Instant::now();
    let ipc_config = crate::ipc::IpcServerConfig {
        agent_id: local_agent_id.to_string(),
        public_key: identity.public_key_base64().to_string(),
        name: config.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        max_client_queue: config.effective_max_client_queue(),
        uptime_secs: Arc::new(move || start.elapsed().as_secs()),
    };

    let (ipc, mut cmd_rx) = IpcServer::bind(
        paths.socket.clone(),
        config.effective_max_ipc_clients(),
        ipc_config,
    )
    .await?;

    // --- Inbound message forwarder (transport → IPC clients) ---
    let mut inbound_rx = transport.subscribe_inbound();
    let ipc_for_inbound = ipc.clone();
    let counters_for_inbound = counters.clone();
    let peer_table_for_inbound = peer_table.clone();
    let cancel_for_inbound = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_for_inbound.cancelled() => break,
                msg = inbound_rx.recv() => {
                    match msg {
                        Ok(envelope) => {
                            counters_for_inbound.received.fetch_add(1, Ordering::Relaxed);
                            if let Some(ref from) = envelope.from {
                                peer_table_for_inbound
                                    .set_connected(from.as_str(), None)
                                    .await;
                            }
                            if let Err(err) = ipc_for_inbound.broadcast_inbound(&envelope).await {
                                warn!(error = %err, "failed broadcasting inbound to IPC clients");
                            }
                        }
                        Err(err) => {
                            warn!(error = %err, "inbound subscription closed");
                            break;
                        }
                    }
                }
            }
        }
    });

    // --- Discovery ---
    let (peer_event_tx, mut peer_event_rx) = mpsc::channel(256);
    {
        let tx = peer_event_tx.clone();
        let peers = config.peers.clone();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            if let Err(err) = run_static_discovery(peers, tx, cancel_clone).await {
                warn!(error = %err, "static discovery failed");
            }
        });
    }
    if !opts.disable_mdns {
        let tx = peer_event_tx.clone();
        let agent_id = local_agent_id.clone();
        let pubkey = identity.public_key_base64().to_string();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            if let Err(err) = run_mdns_discovery(agent_id, pubkey, port, tx, cancel_clone).await {
                warn!(error = %err, "mDNS discovery failed");
            }
        });
    }

    // --- Reconnection tracking ---
    let mut reconnect_map = HashMap::<AgentId, ReconnectState>::new();
    for peer in peer_table.list().await {
        if *local_agent_id < *peer.agent_id {
            reconnect_map.insert(peer.agent_id, ReconnectState::immediate(Instant::now()));
        }
    }

    // --- Timers ---
    let mut save_interval = tokio::time::interval(Duration::from_secs(60));
    let mut stale_interval = tokio::time::interval(Duration::from_secs(5));
    let mut reconnect_interval = tokio::time::interval(Duration::from_secs(1));

    let ctx = DaemonContext {
        ipc: &ipc,
        peer_table: &peer_table,
        transport: &transport,
        local_agent_id: &local_agent_id,
        counters: &counters,
        start,
    };

    // --- Main event loop ---
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("shutdown signal received");
                break;
            }
            maybe_cmd = cmd_rx.recv() => {
                if let Some(cmd) = maybe_cmd
                    && let Err(err) = handle_command(cmd, &ctx).await
                {
                    error!(error = %err, "failed handling IPC command");
                }
            }
            maybe_event = peer_event_rx.recv() => {
                if let Some(event) = maybe_event {
                    handle_peer_event(
                        event,
                        &peer_table,
                        &local_agent_id,
                        &mut reconnect_map,
                    ).await;
                    if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await).await {
                        warn!(error = %err, "failed to persist known peers after discovery event");
                    }
                }
            }
            _ = stale_interval.tick() => {
                let removed = peer_table.remove_stale(crate::peer_table::STALE_TIMEOUT).await;
                if !removed.is_empty() {
                    for id in &removed {
                        reconnect_map.remove(id);
                    }
                    info!(count = removed.len(), "removed stale discovered peers");
                    if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await).await {
                        warn!(error = %err, "failed to persist known peers after stale cleanup");
                    }
                }
            }
            _ = reconnect_interval.tick() => {
                attempt_reconnects(
                    &peer_table,
                    &transport,
                    &local_agent_id,
                    &mut reconnect_map,
                    config.effective_reconnect_max_backoff(),
                    &cancel,
                ).await;
            }
            _ = save_interval.tick() => {
                if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await).await {
                    warn!(error = %err, "failed to persist known peers");
                }
            }
        }
    }

    // --- Shutdown sequence (spec §8) ---
    info!("shutting down...");

    // Signal all background tasks to stop
    cancel.cancel();
    info!("all background tasks signaled for shutdown");

    // Brief drain period for in-flight streams
    tokio::time::sleep(Duration::from_millis(100)).await;

    transport.close_all().await;
    if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await).await
    {
        warn!(error = %err, "failed to save known peers during shutdown");
    }
    ipc.cleanup_socket()?;
    info!("shutdown complete");

    Ok(())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
