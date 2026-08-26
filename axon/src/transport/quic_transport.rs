use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex, atomic::AtomicU64};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, info, warn};

use crate::identity::Identity;
use crate::message::{AgentId, Envelope};
use crate::peer_directory::PeerDirectory;
use crate::transport::PairRequest;

use super::DIAL_TIMEOUT;
use super::connection::{
    SendError, derive_peer_id_from_connection, retry_permitted, run_connection, send_request,
    send_unidirectional,
};
use super::connection_registry::{Admission, ConnectionRegistry, Direction};
use super::reconnect::ReconnectBook;
use super::tls::{build_endpoint, with_handshake_remote_addr};

pub type ResponseHandlerFn = Arc<
    dyn Fn(Arc<Envelope>) -> Pin<Box<dyn Future<Output = Option<Envelope>> + Send>> + Send + Sync,
>;

#[derive(Clone)]
pub struct ConnectionManager {
    endpoint: quinn::Endpoint,
    local_agent_id: AgentId,
    max_connections: usize,
    registry: ConnectionRegistry,
    connecting_locks: Arc<Mutex<HashMap<AgentId, Arc<Mutex<()>>>>>,
    /// Per-peer dial cancellation: revoked on close_peer so in-flight dials
    /// for a removed peer are cancelled, not left running to completion.
    dial_cancels: Arc<Mutex<HashMap<AgentId, CancellationToken>>>,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
    pair_request_tx: broadcast::Sender<PairRequest>,
    connection_semaphore: Arc<Semaphore>,
    cancel: CancellationToken,
    response_handler: Option<ResponseHandlerFn>,
    inbound_read_timeout: Duration,
    tasks: TaskTracker,
    reconnect: ReconnectBook,
    /// Enrollment authority: consulted immediately before admitting any
    /// connection so a revocation that races an in-flight handshake cannot
    /// land a slot.
    directory: PeerDirectory,
    /// PER-PEER enrollment epochs, bumped ONLY by `close_peer` (revocation).
    /// Handshake attempts capture the DIALED peer's epoch BEFORE the handshake
    /// begins and admission re-checks it, so a handshake that raced a
    /// revocation is rejected at admission even if the same peer was
    /// re-enrolled before the attempt finished. Without this epoch, a
    /// pre-revocation outbound handshake or an untracked inbound handshake
    /// could be admitted against freshly restored trust.
    ///
    /// Scoping matters: a GLOBAL epoch would let revoking peer B reject an
    /// otherwise valid in-flight handshake for unrelated peer A. Entries are
    /// created lazily on first bump, so the map grows only for peers that
    /// were actually revoked (local IPC administration), and epochs are never
    /// reused — pruning entries would allow an ABA reuse against restored
    /// trust. Reads take a short std mutex; no await runs under it.
    enrollment_epochs: Arc<StdMutex<HashMap<AgentId, Arc<AtomicU64>>>>,
}

/// Upper bound on one admission-gate evaluation (the registry lock wait plus
/// the directory enrollment lookup). Both critical sections are short and —
/// since peer-directory persistence moved outside the directory lock — free
/// of disk I/O; the bound exists so a pathological stall fails the gate
/// CLOSED instead of stalling connection admission forever. Outbound dials
/// use the caller's whole-exchange budget instead when it is smaller.
pub(crate) const ADMISSION_GATE_BUDGET: Duration = Duration::from_secs(5);

impl ConnectionManager {
    pub async fn bind(
        bind_addr: SocketAddr,
        identity: &Identity,
        max_connections: usize,
        directory: PeerDirectory,
    ) -> Result<Self> {
        Self::bind_cancellable(
            bind_addr,
            identity,
            CancellationToken::new(),
            max_connections,
            Duration::from_secs(15),
            Duration::from_secs(60),
            None,
            Duration::from_secs(10),
            directory,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn bind_cancellable(
        bind_addr: SocketAddr,
        identity: &Identity,
        cancel: CancellationToken,
        max_connections: usize,
        keepalive: Duration,
        idle_timeout: Duration,
        response_handler: Option<ResponseHandlerFn>,
        inbound_read_timeout: Duration,
        directory: PeerDirectory,
    ) -> Result<Self> {
        let local_agent_id = AgentId::parse(identity.agent_id())?;
        let cert = identity.make_quic_certificate()?;
        let (endpoint, inbound_tx, pair_request_tx) = build_endpoint(
            bind_addr,
            &cert,
            directory.pinning_snapshot(),
            keepalive,
            idle_timeout,
        )?;
        let manager = Self {
            endpoint,
            local_agent_id: local_agent_id.clone(),
            max_connections,
            registry: ConnectionRegistry::new(local_agent_id),
            connecting_locks: Arc::new(Mutex::new(HashMap::new())),
            dial_cancels: Arc::new(Mutex::new(HashMap::new())),
            inbound_tx,
            pair_request_tx,
            connection_semaphore: Arc::new(Semaphore::new(max_connections)),
            cancel,
            response_handler,
            inbound_read_timeout,
            tasks: TaskTracker::new(),
            reconnect: ReconnectBook::default(),
            directory,
            enrollment_epochs: Arc::new(StdMutex::new(HashMap::new())),
        };
        manager.spawn_accept_loop();
        Ok(manager)
    }

    pub fn subscribe_inbound(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.inbound_tx.subscribe()
    }

    pub fn subscribe_pair_requests(&self) -> broadcast::Receiver<PairRequest> {
        self.pair_request_tx.subscribe()
    }

    pub async fn has_connection(&self, agent_id: &AgentId) -> bool {
        self.registry.current(agent_id).await.is_some()
    }

    pub async fn connected_count(&self) -> usize {
        self.registry.count().await
    }

    pub async fn close_peer(&self, agent_id: &AgentId, reason: &'static [u8]) {
        // Advance THIS peer's enrollment epoch FIRST so every handshake that
        // began before this revocation — outbound dials tracked here and
        // inbound handshakes captured in the accept loop alike — fails its
        // admission gate even if the peer is re-enrolled moments later. A
        // trusted attempt must start (and capture the new epoch) after
        // revocation. Peer-scoping keeps unrelated in-flight handshakes
        // valid: revoking B must not reject A's handshake.
        self.advance_enrollment_epoch(agent_id);
        // Cancel and retire any in-flight dials for this peer next: the
        // IPC revocation contract requires cancelling attempts, not merely
        // refusing their admission when they finish.
        let dial_cancel = self.dial_cancels.lock().await.remove(agent_id);
        if let Some(token) = dial_cancel {
            token.cancel();
        }
        self.registry.close_peer(agent_id, reason).await;
        self.connecting_locks.lock().await.remove(agent_id);
    }

    pub async fn maintain(&self, directory: &PeerDirectory) {
        let enrolled: std::collections::HashSet<_> =
            directory.enrolled_agent_ids().await.into_iter().collect();
        self.reconnect.retain(&enrolled).await;
        for peer in enrolled {
            if self.registry.current(&peer).await.is_some() {
                continue;
            }
            if directory.dial_targets(&peer).await.is_empty() {
                continue;
            }
            let Some(ticket) = self.reconnect.claim(peer.clone(), Instant::now()).await else {
                continue;
            };
            let manager = self.clone();
            let directory = directory.clone();
            let dial_cancel = manager.dial_token(&peer).await;
            self.tasks.spawn(async move {
                // Observe shutdown AND per-peer revocation: a dial stuck in
                // a handshake must not outlive close_all's join window as a
                // detached task, nor keep dialing after removal.
                tokio::select! {
                    _ = manager.cancel.cancelled() => {
                        // Shutdown/revocation ended the attempt without a dial
                        // outcome: release the ticket or the entry would stay
                        // `in_flight` forever and maintenance could never claim
                        // another attempt for this peer after re-enrollment.
                        manager.reconnect.abandoned(&peer, ticket).await;
                    }
                    _ = dial_cancel.cancelled() => {
                        manager.reconnect.abandoned(&peer, ticket).await;
                    }
                    result = manager.connect_peer(&directory, &peer, Instant::now() + DIAL_TIMEOUT) => {
                        match result {
                            Ok(_) => manager.reconnect.succeeded(&peer, ticket).await,
                            Err(err) => {
                                if let Some(wait) = manager
                                    .reconnect
                                    .failed(&peer, ticket, Instant::now())
                                    .await
                                {
                                    warn!(peer = %peer, error = %err.inner, retry_in = ?wait, "reconnect attempt failed");
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    pub async fn send_to(
        &self,
        directory: &PeerDirectory,
        peer: &AgentId,
        envelope: Envelope,
        request_timeout: Duration,
    ) -> Result<Option<Envelope>, SendError> {
        // A send that collides with cross-dial convergence can fail on a
        // connection that loses the tie-break and is closed moments later
        // (DEC-014). Q-006: failure invalidates the suspect slot, so the
        // exchange redials instead of surfacing a transient error for a
        // reachable peer. One retry against the refreshed authoritative
        // slot.
        //
        // The deadline covers the whole exchange (dial + write + response)
        // and is enforced INSIDE this call, never by an external canceller:
        // a dropped future could skip `send_once`'s retirement of the exact
        // failed connection. Every failure here flows through normal error
        // returns, so retirement is unconditional on failure. Checked add:
        // a hostile `timeout_secs` must overflow into a typed error, not a
        // panic that leaks the caller's reserved send slot.
        let deadline = Instant::now().checked_add(request_timeout).ok_or_else(|| {
            SendError::pre_send_timeout(anyhow!("request timeout exceeds the supported range"))
        })?;
        match self
            .send_once(directory, peer, envelope.clone(), deadline)
            .await
        {
            Ok(response) => Ok(response),
            Err(first_error) => {
                debug!(
                    peer = %peer.as_str(),
                    error = %first_error.inner,
                    "send failed on suspect slot"
                );
                if !retry_permitted(&envelope.kind, &first_error) {
                    // Ambiguous fire-and-forget failure: the message may
                    // already have been delivered and broadcast by the peer,
                    // so a retry could duplicate application delivery.
                    // At-most-once wins; surface the error to the caller.
                    return Err(first_error);
                }
                self.send_once(directory, peer, envelope, deadline).await
            }
        }
    }

    async fn send_once(
        &self,
        directory: &PeerDirectory,
        peer: &AgentId,
        envelope: Envelope,
        deadline: Instant,
    ) -> Result<Option<Envelope>, SendError> {
        let connection = self.connect_peer(directory, peer, deadline).await?;
        let result = if envelope.kind.expects_response() {
            send_request(&connection, envelope, &self.local_agent_id, deadline)
                .await
                .map(Some)
        } else {
            send_unidirectional(&connection, envelope, deadline)
                .await
                .map(|()| None)
        };
        if result.is_err() {
            // Retire only the slot this failed exchange used, telling the
            // peer so its mirror slot clears too. A newer, authoritative
            // replacement (cross-dial winner) must survive untouched.
            self.registry
                .retire_if_current_connection(peer, &connection, b"send failed on suspect slot")
                .await;
        }
        result
    }

    /// Dial with a bounded handshake. All awaits before slot installation
    /// are either quick lock operations or this bounded handshake, so no
    /// cancellation point exists between the admission gate and slot
    /// installation that could leave a half-registered slot.
    pub async fn close_all(&self) {
        self.endpoint.close(0u32.into(), b"shutdown");
        self.registry.close_all().await;
        // Every tracked task observes `cancel` (accept loop, connection
        // loops via run_connection, reconnect dials above), so this join is
        // bounded in practice; the timeout only guards pathological stalls.
        self.tasks.close();
        if tokio::time::timeout(Duration::from_secs(2), self.tasks.wait())
            .await
            .is_err()
        {
            warn!(
                "timed out waiting for connection tasks to stop; \
                 residual tasks observe cancellation and will exit"
            );
        }
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint
            .local_addr()
            .context("failed to get local address")
    }

    fn spawn_accept_loop(&self) {
        let manager = self.clone();
        self.tasks.spawn(async move {
            loop {
                tokio::select! {
                    _ = manager.cancel.cancelled() => {
                        info!("QUIC accept loop shutting down");
                        break;
                    }
                    incoming = manager.endpoint.accept() => {
                        let Some(connecting) = incoming else { break };
                        let remote_addr = connecting.remote_address();
                        // Capture EVERY peer's enrollment epoch BEFORE awaiting
                        // the handshake: inbound handshakes are not tracked by
                        // per-peer dial tokens and the authenticated peer is not
                        // known until the TLS identity is derived, so the
                        // snapshot is what lets admission reject a handshake
                        // that started before ITS OWN peer's revocation
                        // committed, even if that peer was re-enrolled before
                        // the handshake completed. Unrelated peers' epochs are
                        // ignored at admission, so their revocations cannot
                        // interfere.
                        let epochs_at_handshake_start = manager.capture_enrollment_epochs();
                        match with_handshake_remote_addr(remote_addr, connecting.into_future()).await {
                            Ok(connection) => {
                                manager
                                    .accept_connection(connection, epochs_at_handshake_start)
                                    .await
                            }
                            Err(err) => warn!(error = %err, "failed to accept QUIC connection"),
                        }
                    }
                }
            }
        });
    }

    async fn accept_connection(
        &self,
        connection: quinn::Connection,
        epochs_at_handshake_start: HashMap<AgentId, u64>,
    ) {
        let permit = match self.connection_semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                warn!(
                    max = self.max_connections,
                    "rejecting connection: limit reached"
                );
                connection.close(0u32.into(), b"connection limit reached");
                return;
            }
        };
        let peer =
            match derive_peer_id_from_connection(&connection).and_then(|id| AgentId::parse(&id)) {
                Ok(peer) => peer,
                Err(err) => {
                    warn!(error = %err, "failed to derive authenticated peer identity");
                    connection.close(0u32.into(), b"invalid peer identity");
                    return;
                }
            };
        debug!(peer = %peer, remote = ?connection.remote_address(), "accepted QUIC connection");
        // TLS pinning already rejects unknown peers, but a handshake that
        // started before revocation committed can still complete. Two gates
        // run under the registry's admission lock, linearized against
        // `remove_peer`'s `close_peer`: unchanged per-peer enrollment epoch
        // (captured before THIS peer's handshake began) AND current
        // enrollment. A stale handshake either fails the gate or is closed
        // moments later by the revocation itself — never left live, and
        // never admitted against restored trust.
        let captured_epoch = epochs_at_handshake_start.get(&peer).copied().unwrap_or(0);
        let gate_peer = peer.clone();
        let admission = self
            .registry
            .admit_gated(peer.clone(), connection.clone(), Direction::Inbound, || {
                self.admission_gate(gate_peer.clone(), captured_epoch, None)
            })
            .await;
        if let Admission::Accepted { generation } = admission {
            self.spawn_connection_loop(peer, generation, connection, permit);
        }
    }

    fn spawn_connection_loop(
        &self,
        peer: AgentId,
        generation: u64,
        connection: quinn::Connection,
        permit: OwnedSemaphorePermit,
    ) {
        let manager = self.clone();
        self.tasks.spawn(async move {
            let stable_id = connection.stable_id();
            run_connection(
                connection,
                manager.local_agent_id.clone(),
                manager.inbound_tx.clone(),
                manager.cancel.clone(),
                manager.response_handler.clone(),
                manager.inbound_read_timeout,
            )
            .await;
            drop(permit);
            manager
                .registry
                .release_if_current(&peer, generation, stable_id)
                .await;
        });
    }
}

#[path = "quic_transport_dial.rs"]
mod dial;

#[path = "quic_transport_epochs.rs"]
mod epochs;

#[cfg(test)]
#[path = "quic_transport_tests/mod.rs"]
mod tests;
