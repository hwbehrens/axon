use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::{AxonPaths, Config, load_known_peers, save_known_peers};
use crate::discovery::{Discovery, MdnsDiscovery, PeerEvent, StaticDiscovery};
use crate::identity::Identity;
use crate::ipc::{CommandEvent, DaemonReply, IpcCommand, IpcServer, PeerSummary};
use crate::message::{Envelope, MessageKind};
use crate::peer_table::{ConnectionStatus, PeerSource, PeerTable};
use crate::transport::QuicTransport;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct DaemonOptions {
    pub port: Option<u16>,
    pub enable_mdns: bool,
    pub axon_root: Option<PathBuf>,
    pub agent_id: Option<String>,
    pub cancel: Option<CancellationToken>,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Counters {
    sent: AtomicU64,
    received: AtomicU64,
}

#[derive(Debug, Clone)]
struct ReconnectState {
    next_attempt_at: Instant,
    current_backoff: Duration,
}

impl ReconnectState {
    fn immediate(now: Instant) -> Self {
        Self {
            next_attempt_at: now,
            current_backoff: Duration::from_secs(1),
        }
    }

    fn schedule_failure(&mut self, now: Instant) -> Duration {
        let wait = self.current_backoff;
        self.next_attempt_at = now + wait;
        self.current_backoff = std::cmp::min(wait.saturating_mul(2), Duration::from_secs(30));
        wait
    }
}

struct ReplayCache {
    ttl: Duration,
    // std::sync required: used in synchronous contexts only
    seen: StdMutex<HashMap<uuid::Uuid, Instant>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ReplayCacheEntry {
    id: uuid::Uuid,
    seen_at_ms: u64,
}

impl ReplayCache {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            seen: StdMutex::new(HashMap::new()),
        }
    }

    fn load(path: &std::path::Path, ttl: Duration) -> Self {
        let cache = Self::new(ttl);
        if !path.exists() {
            return cache;
        }
        let Ok(data) = std::fs::read_to_string(path) else {
            return cache;
        };
        let Ok(entries) = serde_json::from_str::<Vec<ReplayCacheEntry>>(&data) else {
            return cache;
        };
        let now_ms = crate::message::now_millis();
        let ttl_ms = ttl.as_millis() as u64;
        let Ok(mut seen) = cache.seen.lock() else {
            return cache;
        };
        let now_instant = Instant::now();
        for entry in entries {
            if now_ms.saturating_sub(entry.seen_at_ms) <= ttl_ms {
                // Approximate the Instant based on elapsed time
                let age = Duration::from_millis(now_ms.saturating_sub(entry.seen_at_ms));
                seen.insert(entry.id, now_instant - age);
            }
        }
        drop(seen);
        cache
    }

    fn save(&self, path: &std::path::Path) -> Result<()> {
        let Ok(seen) = self.seen.lock() else {
            return Ok(());
        };
        let now = Instant::now();
        let now_ms = crate::message::now_millis();
        let entries: Vec<ReplayCacheEntry> = seen
            .iter()
            .filter(|(_, ts)| now.saturating_duration_since(**ts) <= self.ttl)
            .map(|(id, ts)| {
                let age_ms = now.saturating_duration_since(*ts).as_millis() as u64;
                ReplayCacheEntry {
                    id: *id,
                    seen_at_ms: now_ms.saturating_sub(age_ms),
                }
            })
            .collect();
        drop(seen);
        let data = serde_json::to_vec_pretty(&entries).context("failed to encode replay cache")?;
        std::fs::write(path, data)
            .with_context(|| format!("failed to write replay cache: {}", path.display()))?;
        Ok(())
    }

    fn is_replay(&self, id: uuid::Uuid, now: Instant) -> bool {
        let Ok(mut seen) = self.seen.lock() else {
            warn!("replay cache lock poisoned; treating as non-replay");
            return false;
        };
        seen.retain(|_, ts| now.saturating_duration_since(*ts) <= self.ttl);
        if seen.contains_key(&id) {
            return true;
        }
        seen.insert(id, now);
        false
    }
}

fn instructive_send_error(peer_id: &str, err: &anyhow::Error) -> String {
    let root_cause = err.root_cause();
    format!(
        "Failed to reach peer {peer_id}: {root_cause}. \
         Check that the peer's daemon is running and reachable."
    )
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
    let local_agent_id = opts
        .agent_id
        .unwrap_or_else(|| identity.agent_id().to_string());

    info!(agent_id = %local_agent_id, port, "starting AXON daemon");

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

    // --- Transport ---
    let bind_addr = format!("0.0.0.0:{port}")
        .parse()
        .context("invalid bind address")?;
    let transport =
        QuicTransport::bind_cancellable(bind_addr, &identity, cancel.clone()).await?;
    // Eagerly populate expected_pubkeys from peer table so inbound connections are pinned
    for peer in peer_table.list().await {
        transport.set_expected_peer(peer.agent_id.clone(), peer.pubkey.clone());
    }

    // --- IPC ---
    let (ipc, mut cmd_rx) = IpcServer::bind(paths.socket.clone()).await?;

    // --- Counters & replay cache ---
    let counters = Arc::new(Counters::default());
    let replay_cache = Arc::new(ReplayCache::load(&paths.replay_cache, Duration::from_secs(300)));
    let start = Instant::now();

    // --- Inbound message forwarder (transport → IPC clients) ---
    let mut inbound_rx = transport.subscribe_inbound();
    let ipc_for_inbound = ipc.clone();
    let counters_for_inbound = counters.clone();
    let peer_table_for_inbound = peer_table.clone();
    let replay_cache_for_inbound = replay_cache.clone();
    let cancel_for_inbound = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_for_inbound.cancelled() => break,
                msg = inbound_rx.recv() => {
                    match msg {
                        Ok(envelope) => {
                            if replay_cache_for_inbound.is_replay(envelope.id, Instant::now()) {
                                warn!(msg_id = %envelope.id, "dropping replayed inbound envelope");
                                continue;
                            }
                            counters_for_inbound.received.fetch_add(1, Ordering::Relaxed);
                            if envelope.kind == MessageKind::Hello {
                                peer_table_for_inbound
                                    .set_connected(&envelope.from, None)
                                    .await;
                            }
                            if let Err(err) = ipc_for_inbound.broadcast_inbound(envelope).await {
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
        let static_discovery = StaticDiscovery::new(config.peers.clone());
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            if let Err(err) = static_discovery.run(tx, cancel_clone).await {
                warn!(error = %err, "static discovery failed");
            }
        });
    }
    if opts.enable_mdns {
        let tx = peer_event_tx.clone();
        let mdns = MdnsDiscovery::new(
            local_agent_id.clone(),
            identity.public_key_base64().to_string(),
            port,
        );
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            if let Err(err) = mdns.run(tx, cancel_clone).await {
                warn!(error = %err, "mDNS discovery failed");
            }
        });
    }

    // --- Reconnection tracking ---
    let mut reconnect_state = HashMap::<String, ReconnectState>::new();
    for peer in peer_table.list().await {
        if *local_agent_id < *peer.agent_id {
            reconnect_state.insert(peer.agent_id, ReconnectState::immediate(Instant::now()));
        }
    }

    // --- Timers ---
    let mut save_interval = tokio::time::interval(Duration::from_secs(60));
    let mut stale_interval = tokio::time::interval(Duration::from_secs(5));
    let mut reconnect_interval = tokio::time::interval(Duration::from_secs(1));
    let mut shutdown = Box::pin(tokio::signal::ctrl_c());

    let ctx = DaemonContext {
        ipc: &ipc,
        peer_table: &peer_table,
        transport: &transport,
        local_agent_id: &local_agent_id,
        counters: &counters,
        replay_cache: &replay_cache,
        start,
    };

    // --- Main event loop ---
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("shutdown signal received");
                break;
            }
            _ = cancel.cancelled() => {
                info!("cancellation token triggered");
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
                        &transport,
                        &local_agent_id,
                        &mut reconnect_state,
                    ).await;
                    if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await) {
                        warn!(error = %err, "failed to persist known peers after discovery event");
                    }
                }
            }
            _ = stale_interval.tick() => {
                let removed = peer_table.remove_stale(crate::peer_table::STALE_TIMEOUT).await;
                if !removed.is_empty() {
                    for id in &removed {
                        reconnect_state.remove(id);
                    }
                    info!(count = removed.len(), "removed stale discovered peers");
                    if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await) {
                        warn!(error = %err, "failed to persist known peers after stale cleanup");
                    }
                }
            }
            _ = reconnect_interval.tick() => {
                attempt_reconnects(
                    &peer_table,
                    &transport,
                    &local_agent_id,
                    &mut reconnect_state,
                ).await;
            }
            _ = save_interval.tick() => {
                if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await) {
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
    if let Err(err) = save_known_peers(&paths.known_peers, &peer_table.to_known_peers().await) {
        warn!(error = %err, "failed to save known peers during shutdown");
    }
    if let Err(err) = replay_cache.save(&paths.replay_cache) {
        warn!(error = %err, "failed to save replay cache during shutdown");
    }
    ipc.cleanup_socket()?;
    info!("shutdown complete");

    Ok(())
}

// ---------------------------------------------------------------------------
// Peer event handling
// ---------------------------------------------------------------------------

async fn handle_peer_event(
    event: PeerEvent,
    peer_table: &PeerTable,
    transport: &QuicTransport,
    local_agent_id: &str,
    reconnect_state: &mut HashMap<String, ReconnectState>,
) {
    let now = Instant::now();

    match event {
        PeerEvent::Discovered {
            agent_id,
            addr,
            pubkey,
        } => {
            if let Some(existing) = peer_table.get(&agent_id).await
                && matches!(existing.source, PeerSource::Static | PeerSource::Cached)
                && existing.pubkey != pubkey
            {
                warn!(
                    peer_id = %agent_id,
                    source = ?existing.source,
                    "ignoring discovered pubkey change for pinned peer"
                );
                if local_agent_id < agent_id.as_str() {
                    reconnect_state
                        .entry(agent_id)
                        .or_insert_with(|| ReconnectState::immediate(now));
                }
                return;
            }

            transport.set_expected_peer(agent_id.clone(), pubkey.clone());
            peer_table
                .upsert_discovered(agent_id.clone(), addr, pubkey)
                .await;

            if local_agent_id < agent_id.as_str() {
                reconnect_state.insert(agent_id, ReconnectState::immediate(now));
            }
        }
        PeerEvent::Lost { agent_id } => {
            peer_table.set_disconnected(&agent_id).await;
            reconnect_state.remove(&agent_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Reconnection with exponential backoff
// ---------------------------------------------------------------------------

async fn attempt_reconnects(
    peer_table: &PeerTable,
    transport: &QuicTransport,
    local_agent_id: &str,
    reconnect_state: &mut HashMap<String, ReconnectState>,
) {
    let now = Instant::now();

    for peer in peer_table.list().await {
        let mut status = peer.status;
        if status == ConnectionStatus::Connected && !transport.has_connection(&peer.agent_id).await
        {
            peer_table.set_disconnected(&peer.agent_id).await;
            status = ConnectionStatus::Disconnected;
        }

        if local_agent_id < peer.agent_id.as_str() && status != ConnectionStatus::Connected {
            reconnect_state
                .entry(peer.agent_id)
                .or_insert_with(|| ReconnectState::immediate(now));
        }
    }

    let attempt_ids: Vec<String> = reconnect_state
        .iter()
        .filter_map(|(id, state)| {
            if state.next_attempt_at <= now {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();

    for agent_id in attempt_ids {
        let Some(peer) = peer_table.get(&agent_id).await else {
            reconnect_state.remove(&agent_id);
            continue;
        };

        if peer.status == ConnectionStatus::Connected
            && transport.has_connection(&agent_id).await
        {
            reconnect_state.remove(&agent_id);
            continue;
        }
        if peer.status == ConnectionStatus::Connected {
            peer_table.set_disconnected(&agent_id).await;
        }

        peer_table
            .set_status(&agent_id, ConnectionStatus::Connecting)
            .await;
        match transport.ensure_connection(&peer).await {
            Ok(conn) => {
                let rtt = conn.rtt().as_secs_f64() * 1000.0;
                peer_table.set_connected(&agent_id, Some(rtt)).await;
                reconnect_state.remove(&agent_id);
            }
            Err(err) => {
                peer_table.set_disconnected(&agent_id).await;
                if let Some(state) = reconnect_state.get_mut(&agent_id) {
                    let wait = state.schedule_failure(now);
                    warn!(
                        peer_id = %agent_id,
                        error = %err,
                        next_attempt_in_secs = wait.as_secs(),
                        "failed reconnect; scheduling backoff retry"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IPC command handler
// ---------------------------------------------------------------------------

struct DaemonContext<'a> {
    ipc: &'a IpcServer,
    peer_table: &'a PeerTable,
    transport: &'a QuicTransport,
    local_agent_id: &'a str,
    counters: &'a Counters,
    replay_cache: &'a ReplayCache,
    start: Instant,
}

async fn handle_command(cmd: CommandEvent, ctx: &DaemonContext<'_>) -> Result<()> {
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
            if **local_agent_id > *to
                && !transport.has_connection(&to).await
            {
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

            let mut envelope = Envelope::new(local_agent_id.to_string(), to.clone(), kind, payload);
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
                        if replay_cache.is_replay(response_envelope.id, Instant::now()) {
                            warn!(msg_id = %response_envelope.id, "dropping replayed response");
                        } else {
                            counters.received.fetch_add(1, Ordering::Relaxed);
                            ipc.broadcast_inbound(response_envelope).await?;
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
                    id: p.agent_id,
                    addr: p.addr.to_string(),
                    status: format!("{:?}", p.status).to_lowercase(),
                    rtt_ms: p.rtt_ms,
                    source: format!("{:?}", p.source).to_lowercase(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_options_default() {
        let opts = DaemonOptions::default();
        assert!(opts.port.is_none());
        assert!(!opts.enable_mdns);
        assert!(opts.axon_root.is_none());
    }

    #[test]
    fn reconnect_backoff_doubles_and_caps() {
        let now = Instant::now();
        let mut state = ReconnectState::immediate(now);
        assert_eq!(state.current_backoff, Duration::from_secs(1));

        state.schedule_failure(now);
        assert_eq!(state.current_backoff, Duration::from_secs(2));

        state.schedule_failure(now);
        assert_eq!(state.current_backoff, Duration::from_secs(4));

        for _ in 0..10 {
            state.schedule_failure(now);
        }
        assert_eq!(state.current_backoff, Duration::from_secs(30));
    }

    #[test]
    fn reconnect_immediate_is_ready() {
        let now = Instant::now();
        let state = ReconnectState::immediate(now);
        assert!(state.next_attempt_at <= now);
    }

    #[test]
    fn replay_cache_marks_duplicates() {
        let cache = ReplayCache::new(Duration::from_secs(10));
        let id = uuid::Uuid::new_v4();
        let now = Instant::now();

        assert!(!cache.is_replay(id, now));
        assert!(cache.is_replay(id, now));
    }

    #[test]
    fn replay_cache_expires_old_entries() {
        let cache = ReplayCache::new(Duration::from_secs(1));
        let id = uuid::Uuid::new_v4();
        let now = Instant::now();

        assert!(!cache.is_replay(id, now));
        assert!(cache.is_replay(id, now));
        assert!(!cache.is_replay(id, now + Duration::from_secs(2)));
    }

    #[test]
    fn replay_cache_different_ids_not_duplicates() {
        let cache = ReplayCache::new(Duration::from_secs(10));
        let now = Instant::now();

        assert!(!cache.is_replay(uuid::Uuid::new_v4(), now));
        assert!(!cache.is_replay(uuid::Uuid::new_v4(), now));
    }
}
