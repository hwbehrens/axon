pub(crate) mod command_handler;
mod lockfile;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use command_handler::{Counters, DaemonContext, handle_command};
use lockfile::DaemonLock;

use crate::config::{AxonPaths, Config};
use crate::discovery::{DiscoveryEvent, run_mdns_discovery};
use crate::identity::Identity;
use crate::ipc::{IpcServer, IpcServerConfig};
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
    let mut daemon_lock = DaemonLock::acquire(&paths.root)?;
    let config = Config::load(&paths.config).await?;
    let port = config.effective_port(opts.port);
    let identity = Identity::load_or_generate(&paths)?;
    let local_agent_id = AgentId::parse(identity.agent_id())?;

    if crate::message::now_millis() == 0 {
        anyhow::bail!("system clock appears invalid; configure system time before starting AXON");
    }
    info!(agent_id = %local_agent_id, port, "starting AXON daemon");

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
                port,
                discovery_tx,
                discovery_cancel,
            )
            .await
        });
    }

    let mut stale_interval = tokio::time::interval(Duration::from_secs(5));
    let mut reconnect_interval = tokio::time::interval(Duration::from_secs(1));
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
    };

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("shutdown signal received");
                break;
            }
            maybe_command = command_rx.recv() => {
                let Some(command) = maybe_command else { break };
                if matches!(&command.command, crate::ipc::IpcCommand::Send { .. }) {
                    // Control commands never consume send capacity: they are
                    // handled inline even when every send slot is busy. Send
                    // capacity is reserved inside `handle_command` itself, so
                    // the budget counts exactly the sends being processed.
                    let command_ctx = ctx.clone();
                    send_tasks.spawn(async move {
                        handle_command(command, &command_ctx).await
                    });
                } else if let Err(err) = handle_command(command, &ctx).await {
                    error!(error = %err, "failed handling IPC command");
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
                    warn!(count, "daemon lagged behind inbound transport events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            pair_request = pair_request_rx.recv() => match pair_request {
                Ok(request) => handle_pair_request(&directory, &ipc, request).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    warn!(count, "daemon lagged behind rejected handshake observations");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            event = discovery_rx.recv() => match event {
                Some(event) => handle_discovery_event(&directory, &ipc, event).await,
                // A closed channel would otherwise be ready-forever and spin.
                None => {
                    warn!("discovery event channel closed; stopping discovery handling");
                    break;
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
                    let request_id = delivery.request_id;
                    if ipc
                        .send_request_event(delivery.client_id, &request)
                        .await
                        .is_err()
                    {
                        broker
                            .fail(
                                request_id,
                                "overloaded",
                                "request handler queue is unavailable",
                                true,
                            )
                            .await;
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
    info!(msg_id = %envelope.id, from, kind = %envelope.kind, "message received");
    let raw = envelope.payload.get();
    debug!(msg_id = %envelope.id, payload = %truncate(raw, 256), "message payload preview");
    trace!(msg_id = %envelope.id, payload = raw, "message payload");
    if let Err(err) = ipc.broadcast_inbound(envelope).await {
        warn!(error = %err, "failed broadcasting inbound message to IPC clients");
    }
}

async fn handle_discovery_event(directory: &PeerDirectory, ipc: &IpcServer, event: DiscoveryEvent) {
    match event {
        DiscoveryEvent::Observed(observation) => {
            observe_and_publish(directory, ipc, observation).await
        }
        DiscoveryEvent::Lost(id) => directory.remove_observation(&id).await,
    }
}

async fn handle_pair_request(directory: &PeerDirectory, ipc: &IpcServer, request: PairRequest) {
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
        Ok(observation) => observe_and_publish(directory, ipc, observation).await,
        Err(err) => warn!(error = %err, "rejected invalid handshake candidate"),
    }
}

async fn observe_and_publish(
    directory: &PeerDirectory,
    ipc: &IpcServer,
    observation: PeerObservation,
) {
    let agent_id = observation.identity.agent_id().clone();
    let source = match observation.source {
        ObservationSource::Mdns => "mdns",
        ObservationSource::Handshake => "handshake",
    };
    let outcome = directory.observe(observation).await;
    match outcome {
        ObserveOutcome::CandidateAdded | ObserveOutcome::CandidateRefreshed => {
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
