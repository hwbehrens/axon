pub(crate) mod command_handler;
mod lockfile;
mod who_can;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use command_handler::{Counters, DaemonContext, handle_command};
use lockfile::DaemonLock;

use crate::config::{AxonPaths, Config};
use crate::discovery::{DiscoveryEvent, run_mdns_discovery};
use crate::identity::Identity;
use crate::ipc::{DaemonReply, IpcErrorCode, IpcServer, IpcServerConfig};
use crate::message::{AgentId, Envelope};
use crate::peer_directory::{
    OBSERVATION_STALE_TIMEOUT, ObservationId, ObservationSource, ObserveOutcome, PeerDirectory,
    PeerObservation, PeerStore, PeerTrust,
};
use crate::request_broker::{BeginRequest, RequestBroker};
use crate::transport::{ConnectionManager, PairRequest, ResponseHandlerFn};

const MAX_CONNECTIONS: usize = 128;
const KEEPALIVE: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const INBOUND_READ_TIMEOUT: Duration = Duration::from_secs(10);
const INBOUND_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_IPC_CLIENTS: usize = 64;
const MAX_CLIENT_QUEUE: usize = 1024;
const MAX_INFLIGHT_SENDS: usize = 256;
const MAX_INFLIGHT_WHO_CAN: usize = 8; // parked/running who_can; overflow rejected (I8)
// Untrusted candidate broadcasts are rate-limited to protect client queues (I13).
const CANDIDATE_BROADCAST_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Default)]
pub struct DaemonOptions {
    pub port: Option<u16>,
    pub disable_mdns: bool,
    pub axon_root: Option<PathBuf>,
    pub cancel: Option<CancellationToken>,
    /// Overrides MAX_INFLIGHT_SENDS; intended for tests that need a small,
    /// deterministic saturation budget.
    pub max_inflight_sends: Option<usize>,
}

pub async fn run_daemon(opts: DaemonOptions) -> Result<()> {
    let paths = match opts.axon_root {
        Some(ref root) => AxonPaths::from_root(root.clone()),
        None => AxonPaths::discover()?,
    };
    paths.ensure_root_exists()?;
    paths.reject_legacy_peer_state()?;
    let mut daemon_lock = DaemonLock::acquire(&paths.daemon_lock)?;
    let config = Config::load(&paths.config).await?;
    let port = config.effective_port(opts.port);
    let identity = Identity::load_or_generate(&paths)?;
    let local_agent_id = AgentId::parse(identity.agent_id())?;

    if crate::message::now_millis() == 0 {
        anyhow::bail!("system clock appears invalid; configure system time before starting AXON");
    }
    let cancel = opts.cancel.unwrap_or_default();
    let start = Instant::now();
    let counters = Arc::new(Counters::default());
    let directory =
        PeerDirectory::load(local_agent_id.clone(), PeerStore::new(paths.peers.clone())).await?;

    let ipc_config = IpcServerConfig {
        agent_id: local_agent_id.to_string(),
        public_key: identity.public_key_base64().to_string(),
        name: config.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        max_client_queue: MAX_CLIENT_QUEUE,
        uptime_secs: Arc::new(move || start.elapsed().as_secs()),
    };
    let (ipc, mut command_rx) =
        IpcServer::bind(paths.socket.clone(), MAX_IPC_CLIENTS, ipc_config).await?;
    let broker = RequestBroker::new(local_agent_id.clone());
    let response_handler = make_response_handler(ipc.clone(), broker.clone());

    let bind_addr = format!("0.0.0.0:{port}")
        .parse()
        .context("invalid QUIC bind address")?;
    let transport = match ConnectionManager::bind_cancellable(
        bind_addr,
        &identity,
        cancel.clone(),
        MAX_CONNECTIONS,
        KEEPALIVE,
        IDLE_TIMEOUT,
        Some(response_handler),
        INBOUND_READ_TIMEOUT,
        directory.clone(),
    )
    .await
    {
        Ok(transport) => transport,
        Err(error) => {
            if let Err(cleanup_error) = ipc.shutdown().await {
                warn!(error = %cleanup_error, "failed cleaning IPC after transport startup error");
            }
            return Err(error);
        }
    };

    // Advertise the bound port, not `--port 0` (nit).
    let bound_port = transport.local_addr().map_or(port, |addr| addr.port());
    info!(agent_id = %local_agent_id, port = bound_port, "starting AXON daemon");

    let mut inbound_rx = transport.subscribe_inbound();
    let mut pair_request_rx = transport.subscribe_pair_requests();
    let mut disconnect_rx = ipc.subscribe_disconnects();
    let (discovery_tx, mut discovery_rx) = mpsc::channel(256);
    let mut tasks = JoinSet::new();
    let mut send_tasks = JoinSet::new();
    tasks.spawn(wait_for_shutdown_signal(cancel.clone()));
    if !opts.disable_mdns {
        let discovery_cancel = cancel.clone();
        let advertised_id = local_agent_id.clone();
        let advertised_key = identity.public_key_base64().to_string();
        tasks.spawn(async move {
            run_mdns_discovery(
                advertised_id,
                advertised_key,
                bound_port,
                discovery_tx,
                discovery_cancel,
            )
            .await
        });
    }

    let mut stale_interval = tokio::time::interval(Duration::from_secs(5));
    let mut reconnect_interval = tokio::time::interval(Duration::from_secs(1));
    let who_can_budget = Arc::new(Semaphore::new(MAX_INFLIGHT_WHO_CAN));
    let mut discovery_open = true;
    let mut last_candidate_broadcast: Option<Instant> = None;
    let ctx = DaemonContext {
        ipc: ipc.clone(),
        directory: directory.clone(),
        transport: transport.clone(),
        broker: broker.clone(),
        local_agent_id: local_agent_id.clone(),
        counters: counters.clone(),
        inflight_sends: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_inflight_sends: opts.max_inflight_sends.unwrap_or(MAX_INFLIGHT_SENDS),
        start,
        manifest_cache: crate::manifest::ManifestCache::new(),
        who_can_gate: Arc::new(tokio::sync::Mutex::new(())),
    };

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("shutdown signal received");
                break;
            }
            maybe_command = command_rx.recv() => {
                let Some(command) = maybe_command else { break };
                let cmd = command.command.cmd_name();
                let client_id = command.client_id;
                // `who_can` awaits network work on the task set; budgeted below (I8).
                let permit = if cmd == "who_can" {
                    match reserve_who_can(
                        &who_can_budget,
                        command.command.req_id().map(str::to_string),
                    ) {
                        Ok(permit) => Some(permit),
                        Err(reply) => {
                            if let Err(err) = ipc.send_reply(client_id, &reply).await {
                                warn!(error = %err, "failed sending who_can capacity rejection");
                            }
                            None
                        }
                    }
                } else {
                    None
                };
                if cmd == "send" || permit.is_some() {
                    let command_ctx = ctx.clone();
                    send_tasks.spawn(async move {
                        let _permit = permit;
                        handle_command(command, &command_ctx).await
                    });
                } else if cmd != "who_can" {
                    if let Err(err) = handle_command(command, &ctx).await {
                        error!(error = %err, "failed handling IPC command");
                    }
                    if cmd == "serve" {
                        release_lease_if_client_gone(&ipc, &broker, client_id).await;
                    }
                }
            }
            completed = send_tasks.join_next(), if !send_tasks.is_empty() => {
                match completed {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(err))) => warn!(error = %err, "IPC send task failed"),
                    Some(Err(err)) => warn!(error = %err, "IPC send task panicked"),
                    None => {}
                }
            }
            inbound = inbound_rx.recv() => match inbound {
                Ok(envelope) => handle_inbound(&ipc, &counters, &envelope).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    // Make the gap explicit and reconcile clients so lost
                    // disconnects cannot pin a lease (I16).
                    warn!(count, "inbound transport event gap; clients may have missed events");
                    let live: std::collections::HashSet<_> =
                        ipc.connected_client_ids().await.into_iter().collect();
                    broker.reconcile_clients(&live).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            pair_request = pair_request_rx.recv() => match pair_request {
                Ok(request) => {
                    handle_pair_request(&directory, &ipc, request, &mut last_candidate_broadcast)
                        .await
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    warn!(count, "daemon lagged behind rejected handshake observations");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            event = discovery_rx.recv(), if discovery_open => match event {
                Some(DiscoveryEvent::Observed(observation)) => {
                    observe_and_publish(
                        &directory,
                        &ipc,
                        observation,
                        &mut last_candidate_broadcast,
                    )
                    .await
                }
                Some(DiscoveryEvent::Lost(id)) => directory.remove_observation(&id).await,
                // Closed is ready-forever and would spin; degraded, not fatal (I10).
                None => {
                    warn!("discovery event channel closed; continuing without discovery");
                    discovery_open = false;
                }
            },
            disconnected = disconnect_rx.recv() => match disconnected {
                Ok(client_id) => broker.disconnect(client_id).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    // Lagged deliveries lose specific client ids; reconcile
                    // lease and pending-request state against live clients so
                    // a disconnected handler cannot retain its lease.
                    warn!(count, "IPC disconnect notifications lagged; reconciling");
                    let live: std::collections::HashSet<_> =
                        ipc.connected_client_ids().await.into_iter().collect();
                    broker.reconcile_clients(&live).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            _ = stale_interval.tick() => {
                let removed = directory
                    .expire_observations(Instant::now(), OBSERVATION_STALE_TIMEOUT)
                    .await;
                if !removed.is_empty() {
                    debug!(count = removed.len(), "expired stale peer candidates");
                }
            }
            _ = reconnect_interval.tick() => {
                transport.maintain(&directory).await;
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                match completed {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(err))) => warn!(error = %err, "daemon background task stopped"),
                    Some(Err(err)) => warn!(error = %err, "daemon background task panicked"),
                    None => {}
                }
            }
        }
    }

    info!("shutting down AXON daemon");
    cancel.cancel();
    transport.close_all().await;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while send_tasks.join_next().await.is_some() {}
    })
    .await;
    send_tasks.abort_all();
    while send_tasks.join_next().await.is_some() {}
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    let cleanup_result = ipc.shutdown().await;
    if let Err(err) = daemon_lock.release() {
        warn!(error = %err, "failed to remove daemon lock file");
    }
    cleanup_result?;
    info!("shutdown complete");
    Ok(())
}

fn make_response_handler(ipc: IpcServer, broker: RequestBroker) -> ResponseHandlerFn {
    Arc::new(move |request| {
        let ipc = ipc.clone();
        let broker = broker.clone();
        Box::pin(async move {
            match broker.begin(request.clone(), INBOUND_REQUEST_TIMEOUT).await {
                BeginRequest::Respond(response) => Some(response),
                BeginRequest::Deliver(delivery) => {
                    if ipc
                        .send_request_event(delivery.client_id, &request)
                        .await
                        .is_err()
                    {
                        // Release the gone/overflow-closed handler's lease (B1).
                        broker.disconnect(delivery.client_id).await;
                    }
                    Some(
                        broker
                            .await_response(delivery, INBOUND_REQUEST_TIMEOUT)
                            .await,
                    )
                }
            }
        })
    })
}

async fn handle_inbound(ipc: &IpcServer, counters: &Counters, envelope: &Envelope) {
    counters.received.fetch_add(1, Ordering::Relaxed);
    let from = envelope.from.as_ref().map_or("unknown", AgentId::as_str);
    let kind = truncate(envelope.kind.as_str(), 64);
    info!(msg_id = %envelope.id, from, kind = %kind, "message received");
    let raw = envelope.payload.get();
    debug!(msg_id = %envelope.id, payload = %truncate(raw, 256), "message payload preview");
    trace!(msg_id = %envelope.id, payload = raw, "message payload");
    if let Err(err) = ipc.broadcast_inbound(envelope).await {
        warn!(error = %err, "failed broadcasting inbound message to IPC clients");
    }
}

async fn handle_pair_request(
    directory: &PeerDirectory,
    ipc: &IpcServer,
    request: PairRequest,
    last_broadcast: &mut Option<Instant>,
) {
    let result = (|| -> Result<PeerObservation> {
        let agent_id = AgentId::parse(&request.agent_id)?;
        let endpoint = request.addr.as_deref().and_then(|addr| addr.parse().ok());
        let id = ObservationId::new(format!(
            "handshake:{}:{}",
            agent_id,
            request.addr.as_deref().unwrap_or("unknown")
        ))?;
        PeerObservation::new(
            id,
            agent_id,
            &request.pubkey,
            endpoint,
            None,
            ObservationSource::Handshake,
        )
    })();
    match result {
        Ok(observation) => observe_and_publish(directory, ipc, observation, last_broadcast).await,
        Err(err) => warn!(error = %err, "rejected invalid handshake candidate"),
    }
}

async fn observe_and_publish(
    directory: &PeerDirectory,
    ipc: &IpcServer,
    observation: PeerObservation,
    last_broadcast: &mut Option<Instant>,
) {
    let agent_id = observation.identity.agent_id().clone();
    let source = match observation.source {
        ObservationSource::Mdns => "mdns",
        ObservationSource::Handshake => "handshake",
    };
    let outcome = directory.observe(observation).await;
    match outcome {
        ObserveOutcome::CandidateAdded | ObserveOutcome::CandidateRefreshed => {
            // Untrusted candidates share client queues; rate-limit (I13).
            if !should_broadcast_candidate(last_broadcast, Instant::now()) {
                return;
            }
            if let Some(peer) = directory.list().await.into_iter().find(|peer| {
                peer.trust == PeerTrust::Candidate && peer.identity.agent_id() == &agent_id
            }) {
                let locators = peer
                    .observed_endpoints
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                if let Err(err) = ipc
                    .broadcast_peer_candidate(
                        peer.identity.agent_id().as_str(),
                        peer.identity.public_key(),
                        locators,
                        source,
                    )
                    .await
                {
                    warn!(error = %err, "failed broadcasting peer candidate");
                }
            }
        }
        ObserveOutcome::IdentityConflict => {
            warn!(peer = %agent_id, "rejected conflicting peer identity observation");
        }
        ObserveOutcome::LocatorConflict => {
            warn!(peer = %agent_id, "quarantined conflicting peer locator observation");
        }
        ObserveOutcome::CapacityReached => {
            warn!(peer = %agent_id, "peer observation capacity reached");
        }
        ObserveOutcome::IgnoredSelf | ObserveOutcome::EnrolledPeerRefreshed => {}
    }
}

/// Release the handler lease if its client is already gone (B1).
async fn release_lease_if_client_gone(ipc: &IpcServer, broker: &RequestBroker, client_id: u64) {
    if !ipc.connected_client_ids().await.contains(&client_id) {
        broker.disconnect(client_id).await;
    }
}

/// Reserve a `who_can` slot or return typed backpressure (I8).
fn reserve_who_can(
    budget: &Arc<Semaphore>,
    req_id: Option<String>,
) -> std::result::Result<tokio::sync::OwnedSemaphorePermit, Box<DaemonReply>> {
    budget.clone().try_acquire_owned().map_err(|_| {
        Box::new(DaemonReply::Error {
            ok: false,
            error: IpcErrorCode::SendCapacityExceeded,
            message: "too many concurrent who_can queries; retry shortly".to_string(),
            req_id,
        })
    })
}

/// Suppress candidate broadcasts that arrive inside the rate window (I13).
fn should_broadcast_candidate(last: &mut Option<Instant>, now: Instant) -> bool {
    if last.is_some_and(|at| now.saturating_duration_since(at) < CANDIDATE_BROADCAST_INTERVAL) {
        return false;
    }
    *last = Some(now);
    true
}

fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        input.to_string()
    } else {
        let boundary = input.floor_char_boundary(max);
        format!("{}…", &input[..boundary])
    }
}

#[cfg(unix)]
async fn wait_for_shutdown_signal(cancel: CancellationToken) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm =
        signal(SignalKind::terminate()).context("failed to install SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("failed to install SIGINT handler")?;
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = sigterm.recv() => cancel.cancel(),
        _ = sigint.recv() => cancel.cancel(),
    }
    Ok(())
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal(cancel: CancellationToken) -> Result<()> {
    tokio::select! {
        _ = cancel.cancelled() => {}
        result = tokio::signal::ctrl_c() => {
            result.context("failed to install Ctrl-C handler")?;
            cancel.cancel();
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
