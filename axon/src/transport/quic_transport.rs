use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, warn};

#[cfg(test)]
use revoke::RevocationPause;

use crate::identity::Identity;
use crate::message::{AgentId, Envelope};
use crate::peer_directory::PeerDirectory;
use crate::transport::PairRequest;

use super::DIAL_TIMEOUT;
use super::connection::{SendError, retry_permitted, send_request, send_unidirectional};
use super::connection_registry::ConnectionRegistry;
use super::reconnect::ReconnectBook;
use super::tls::build_endpoint;

pub type ResponseHandlerFn = Arc<
    dyn Fn(Arc<Envelope>) -> Pin<Box<dyn Future<Output = Option<Envelope>> + Send>> + Send + Sync,
>;

/// Inbound-envelope receiver that records broadcast gaps.
///
/// `recv` has exactly the same contract as
/// [`broadcast::Receiver::recv`], including surfacing `RecvError::Lagged(n)`
/// for evicted envelopes; it additionally folds `n` into the manager's gap
/// counter so a consumer can observe the loss even if it only logs the
/// error. Every other [`broadcast::Receiver`] method stays available through
/// `Deref`, so subscriber code written against the raw receiver keeps
/// working.
///
/// Created by [`ConnectionManager::subscribe_inbound_counted`]; the plain
/// [`ConnectionManager::subscribe_inbound`] API is unchanged for callers
/// that do not opt into gap accounting.
pub struct InboundReceiver {
    rx: broadcast::Receiver<Arc<Envelope>>,
    gaps: Arc<AtomicU64>,
}

impl InboundReceiver {
    pub async fn recv(&mut self) -> Result<Arc<Envelope>, broadcast::error::RecvError> {
        match self.rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                self.gaps.fetch_add(missed, Ordering::Relaxed);
                Err(broadcast::error::RecvError::Lagged(missed))
            }
            other => other,
        }
    }
}

impl std::ops::Deref for InboundReceiver {
    type Target = broadcast::Receiver<Arc<Envelope>>;

    fn deref(&self) -> &Self::Target {
        &self.rx
    }
}

impl std::ops::DerefMut for InboundReceiver {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rx
    }
}

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
    /// Bounds concurrent PRE-AUTHENTICATION inbound handshakes. One stalled
    /// handshake must not block later inbound connections, and unbounded
    /// handshake tasks must not accumulate, so each accepted `Incoming`
    /// holds one permit until its handshake finishes; beyond the bound the
    /// incoming connection is refused immediately. The authoritative
    /// connection limit stays on `connection_semaphore` (checked after
    /// mTLS completes).
    handshake_semaphore: Arc<Semaphore>,
    /// Cumulative inbound envelopes evicted from counted subscribers' views
    /// (see [`Self::subscribe_inbound_counted`]); lets the daemon surface
    /// broadcast loss instead of only logging it.
    inbound_gaps: Arc<AtomicU64>,
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
    /// Test-only determinism gate, scoped PER MANAGER (never global, so
    /// parallel tests cannot interfere): when armed, this manager's
    /// in-flight revocations pause between the directory commit and
    /// transport teardown, letting tests cancel the caller or start
    /// `close_all` inside the pairing window deterministically.
    #[cfg(test)]
    revocation_pause: Arc<RevocationPause>,
}

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
            handshake_semaphore: Arc::new(Semaphore::new(max_connections)),
            inbound_gaps: Arc::new(AtomicU64::new(0)),
            cancel,
            response_handler,
            inbound_read_timeout,
            tasks: TaskTracker::new(),
            reconnect: ReconnectBook::default(),
            directory,
            enrollment_epochs: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            revocation_pause: Arc::new(RevocationPause::default()),
        };
        manager.spawn_accept_loop();
        Ok(manager)
    }

    pub fn subscribe_inbound(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.inbound_tx.subscribe()
    }

    /// Like [`Self::subscribe_inbound`], but the returned receiver folds
    /// broadcast lag into a shared gap counter. `recv` reports the same
    /// `RecvError::Lagged(n)` a raw receiver does, so a consumer can both
    /// observe and count evictions.
    pub fn subscribe_inbound_counted(&self) -> InboundReceiver {
        InboundReceiver {
            rx: self.inbound_tx.subscribe(),
            gaps: self.inbound_gaps.clone(),
        }
    }

    /// Cumulative inbound envelopes that counted subscribers observed as
    /// evicted by broadcast lag. Zero until a counted subscription is used;
    /// the daemon can poll this after a `RecvError::Lagged` to reconcile
    /// and notify clients about delivery gaps instead of only warning.
    pub fn inbound_gap_count(&self) -> u64 {
        self.inbound_gaps.load(Ordering::Relaxed)
    }

    pub fn subscribe_pair_requests(&self) -> broadcast::Receiver<PairRequest> {
        self.pair_request_tx.subscribe()
    }

    pub async fn has_connection(&self, agent_id: &AgentId) -> bool {
        self.registry.live_slot(agent_id).await.is_some()
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
            if self.registry.live_slot(&peer).await.is_some() {
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
                // A retry can only make progress while the whole-exchange
                // budget still has time left. Once it is exhausted, a redial
                // could only fail with a generic budget error that would
                // replace this informative failure; preserve the first error.
                if Instant::now() >= deadline {
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
}

#[path = "quic_transport_accept.rs"]
mod accept;

#[path = "quic_transport_dial.rs"]
mod dial;

#[path = "quic_transport_epochs.rs"]
mod epochs;

#[path = "quic_transport_revoke.rs"]
mod revoke;

#[cfg(test)]
#[path = "quic_transport_tests/mod.rs"]
mod tests;
