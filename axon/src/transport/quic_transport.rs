use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
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
            cancel,
            response_handler,
            inbound_read_timeout,
            tasks: TaskTracker::new(),
            reconnect: ReconnectBook::default(),
            directory,
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
        // Cancel and retire any in-flight dials for this peer first: the
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
                    _ = manager.cancel.cancelled() => {}
                    _ = dial_cancel.cancelled() => {}
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
        // returns, so retirement is unconditional on failure.
        let deadline = Instant::now() + request_timeout;
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
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        let result = if envelope.kind.expects_response() {
            send_request(&connection, envelope, &self.local_agent_id, remaining)
                .await
                .map(Some)
        } else {
            send_unidirectional(&connection, envelope, remaining)
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
                        match with_handshake_remote_addr(remote_addr, connecting.into_future()).await {
                            Ok(connection) => manager.accept_connection(connection).await,
                            Err(err) => warn!(error = %err, "failed to accept QUIC connection"),
                        }
                    }
                }
            }
        });
    }

    async fn accept_connection(&self, connection: quinn::Connection) {
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
        // started before revocation committed can still complete. The
        // enrollment gate below runs under the registry's admission lock,
        // so it is linearized against `remove_peer`'s `close_peer`: a
        // handshake racing revocation either fails the gate or is closed
        // moments later by the revocation itself — never left live.
        let admission = self
            .registry
            .admit_gated(peer.clone(), connection.clone(), Direction::Inbound, || {
                self.directory.is_enrolled(&peer)
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

#[cfg(test)]
#[path = "quic_transport_tests/mod.rs"]
mod tests;
