//! Inbound accept path for [`ConnectionManager`]: a non-blocking accept
//! loop, bounded pre-authentication handshakes, and gated admission plus
//! connection-task ownership.
//!
//! Split from `quic_transport.rs` for file-length limits. This is a child
//! module, so the inherent `impl` below retains access to the manager's
//! private fields.

use std::collections::HashMap;
use std::future::IntoFuture;
use std::net::SocketAddr;

use tokio::sync::OwnedSemaphorePermit;
use tracing::{debug, info, warn};

use crate::message::AgentId;

use super::super::connection::{derive_peer_id_from_connection, run_connection};
use super::super::connection_registry::{Admission, Direction};
use super::super::tls::with_handshake_remote_addr;
use super::ConnectionManager;

impl ConnectionManager {
    /// Spawn the accept loop.
    ///
    /// The loop NEVER awaits a handshake inline: every accepted `Incoming`
    /// is handed to its own tracked task, so one stalled (half-open,
    /// unauthenticated) handshake cannot delay later inbound connections.
    /// A separate pre-authentication semaphore bounds how many handshakes
    /// may be in flight at once; beyond that bound an incoming connection
    /// is refused immediately instead of accumulating unbounded tasks.
    pub(super) fn spawn_accept_loop(&self) {
        let manager = self.clone();
        self.tasks.spawn(async move {
            loop {
                tokio::select! {
                    _ = manager.cancel.cancelled() => {
                        info!("QUIC accept loop shutting down");
                        break;
                    }
                    incoming = manager.endpoint.accept() => {
                        let Some(incoming) = incoming else { break };
                        // Bound concurrent pre-authentication handshakes. The
                        // authoritative connection limit stays where it is
                        // (after mTLS completes, when a QUIC close reason can
                        // actually be sent); this bound protects the accept
                        // path while a handshake is still unauthenticated.
                        let permit = match manager.handshake_semaphore.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                warn!(
                                    max = manager.max_connections,
                                    "rejecting handshake: pre-auth handshake limit reached"
                                );
                                incoming.refuse();
                                continue;
                            }
                        };
                        let remote_addr = incoming.remote_address();
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
                        manager.spawn_handshake(
                            incoming,
                            remote_addr,
                            epochs_at_handshake_start,
                            permit,
                        );
                    }
                }
            }
        });
    }

    /// Await one inbound handshake on its own tracked task, then run gated
    /// admission and (on success) the connection loop.
    ///
    /// The pre-auth permit is dropped as soon as the handshake finishes, so
    /// it never spans the authenticated connection's lifetime; the
    /// authoritative `connection_semaphore` remains the only connection
    /// limit.
    fn spawn_handshake(
        &self,
        incoming: quinn::Incoming,
        remote_addr: SocketAddr,
        epochs_at_handshake_start: HashMap<AgentId, u64>,
        permit: OwnedSemaphorePermit,
    ) {
        let manager = self.clone();
        self.tasks.spawn(async move {
            let handshake = tokio::select! {
                _ = manager.cancel.cancelled() => {
                    // Shutdown: dropping the handshake future tears it down.
                    return;
                }
                result = with_handshake_remote_addr(remote_addr, incoming.into_future()) => result,
            };
            drop(permit);
            match handshake {
                Ok(connection) => {
                    manager
                        .accept_connection(connection, epochs_at_handshake_start)
                        .await
                }
                Err(err) => warn!(error = %err, "failed to accept QUIC connection"),
            }
        });
    }

    pub(super) async fn accept_connection(
        &self,
        connection: quinn::Connection,
        epochs_at_handshake_start: HashMap<AgentId, u64>,
    ) {
        // The limit is intentionally enforced only after mTLS completes:
        // quinn can deliver a close reason only on an established
        // connection, and admission needs the authenticated peer identity.
        // The pre-auth `handshake_semaphore` bounds the unauthenticated
        // window without moving this authoritative check.
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
        // run under the registry's admission lock: unchanged per-peer
        // enrollment epoch (captured before THIS peer's handshake began) AND
        // current enrollment in the published pins. The epoch half
        // linearizes against `close_peer`'s epoch bump; the enrollment half
        // against the pin publication that completes `remove_peer`. A stale
        // handshake either fails the gate or is closed moments later by the
        // revocation itself — never left live, and never admitted against
        // restored trust.
        let captured_epoch = epochs_at_handshake_start.get(&peer).copied().unwrap_or(0);
        let gate_peer = peer.clone();
        let admission = self
            .registry
            .admit_gated(
                peer.clone(),
                connection.clone(),
                Direction::Inbound,
                self.admission_gate(gate_peer, captured_epoch),
            )
            .await;
        if let Admission::Accepted { generation } = admission {
            self.spawn_connection_loop(peer, generation, connection, permit);
        }
    }

    pub(super) fn spawn_connection_loop(
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
