//! Outbound dial path for [`ConnectionManager`]: authoritative slot lookup,
//! bounded handshakes, and per-peer dial cancellation.
//!
//! Split from `quic_transport.rs` for file-length limits. This is a child
//! module, so the inherent `impl` below retains access to the manager's
//! private fields.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use tokio_util::sync::CancellationToken;

use crate::message::AgentId;
use crate::peer_directory::{DialTarget, PeerDirectory};

use super::super::DIAL_TIMEOUT;
use super::super::DialPeer;
use super::super::connection::SendError;
use super::super::connection::derive_peer_id_from_connection;
use super::super::connection_registry::{Admission, Direction};
use super::super::tls::with_handshake_remote_addr;
use super::ConnectionManager;
use tokio::sync::Mutex;

impl ConnectionManager {
    /// The current per-peer dial token, creating one if absent.
    pub(super) async fn dial_token(&self, peer: &AgentId) -> CancellationToken {
        self.dial_cancels
            .lock()
            .await
            .entry(peer.clone())
            .or_default()
            .clone()
    }

    pub(super) async fn connect_peer(
        &self,
        directory: &PeerDirectory,
        peer: &AgentId,
        deadline: Instant,
    ) -> Result<quinn::Connection, SendError> {
        if let Some(existing) = self.registry.live_slot(peer).await {
            return Ok(existing);
        }
        let dial_cancel = self.dial_token(peer).await;
        // Peer lookup shares the directory lock with persistence. Persistence
        // no longer runs disk I/O under that lock, but the wait is still part
        // of the exchange budget: a contended directory must never push the
        // request past its `timeout_secs` deadline.
        let targets = tokio::time::timeout(
            super::super::connection::remaining_budget(deadline, "peer lookup")?,
            directory.dial_targets(peer),
        )
        .await
        .map_err(|_| {
            SendError::pre_send_timeout(anyhow!("peer lookup for {peer} exceeded send budget"))
        })?;
        let mut addresses = Vec::new();
        let mut last_error = None;
        for target in targets {
            match target {
                DialTarget::Observed(address) => addresses.push(address),
                DialTarget::Configured(locator) => {
                    // DNS resolution is unbounded by nature; bound it with
                    // the remaining budget so a stalled resolver cannot
                    // exceed the exchange deadline.
                    let Some(resolve_budget) = deadline.checked_duration_since(Instant::now())
                    else {
                        last_error = Some(SendError::pre_send_timeout(anyhow!(
                            "send budget exhausted before resolving {:?}",
                            locator
                        )));
                        break;
                    };
                    // DNS resolution is unbounded by nature; bound it with
                    // the remaining budget so a stalled resolver cannot
                    // exceed the exchange deadline, and make it
                    // cancellation-aware so revocation tears the wait down
                    // immediately instead of after the timeout elapses. The
                    // resolver runs on `spawn_blocking`, so an abandoned
                    // worker thread may finish in the background; its result
                    // is simply dropped — the dial never consumes it.
                    let outcome = tokio::select! {
                        _ = dial_cancel.cancelled() => None,
                        result = tokio::time::timeout(resolve_budget, locator.resolve()) => {
                            Some(result)
                        }
                    };
                    match outcome {
                        None => {
                            last_error = Some(SendError::pre_send(anyhow!(
                                "dial to peer {peer} was cancelled during resolution"
                            )));
                            break;
                        }
                        Some(Ok(Ok(resolved))) => addresses.extend(resolved),
                        Some(Ok(Err(err))) => last_error = Some(SendError::pre_send(err)),
                        Some(Err(_)) => {
                            last_error = Some(SendError::pre_send_timeout(anyhow!(
                                "locator resolution exceeded send budget"
                            )));
                        }
                    }
                }
            }
        }
        addresses.sort();
        addresses.dedup();
        for addr in addresses {
            if dial_cancel.is_cancelled() {
                return Err(SendError::pre_send(anyhow!(
                    "dial to peer {peer} was cancelled"
                )));
            }
            if deadline.checked_duration_since(Instant::now()).is_none() {
                last_error = Some(SendError::pre_send_timeout(anyhow!(
                    "send budget exhausted before dialing {addr}"
                )));
                break;
            };
            match self
                .dial(
                    &DialPeer {
                        agent_id: peer.clone(),
                        addr,
                    },
                    deadline,
                )
                .await
            {
                Ok(connection) => return Ok(connection),
                Err(err) => last_error = Some(SendError::pre_send(err)),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            SendError::pre_send(anyhow!("peer {peer} has no usable dial target"))
        }))
    }

    pub async fn ensure_connection(&self, peer: &DialPeer) -> Result<quinn::Connection> {
        self.dial(peer, Instant::now() + DIAL_TIMEOUT).await
    }

    /// Dial with a bounded handshake under the whole-exchange `deadline`.
    /// Every await here recomputes the remaining budget: the per-peer dial
    /// lock, the handshake, and the admission gate all share one deadline,
    /// so a caller's 1-second exchange can never spend its whole budget on
    /// the lock wait and then receive a fresh full budget for the handshake.
    async fn dial(&self, peer: &DialPeer, deadline: Instant) -> Result<quinn::Connection> {
        if let Some(existing) = self.registry.live_slot(&peer.agent_id).await {
            return Ok(existing);
        }
        // Attempt token: capture THIS peer's enrollment epoch before anything
        // can block. Admission re-checks it, so a handshake that raced a
        // revocation of the same peer is rejected even if the peer was
        // re-enrolled in the meantime — a trusted attempt must start after
        // revocation committed. Other peers' epochs are irrelevant here.
        let epoch_at_dial_start = self.peer_enrollment_epoch(&peer.agent_id);
        let dial_cancel = self.dial_token(&peer.agent_id).await;
        let peer_lock = {
            let mut locks = self.connecting_locks.lock().await;
            locks
                .entry(peer.agent_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        // Waiting for the per-peer dial lock is part of the exchange budget:
        // without this bound a contended lock could stall the caller past
        // its deadline indefinitely. The wait is also cancellation-aware:
        // `close_peer` (reached via `revoke_peer`) retires the dial token,
        // so a queued dial for a revoked peer aborts immediately rather
        // than taking its turn and discovering the revocation afterwards.
        let _guard = tokio::select! {
            _ = dial_cancel.cancelled() => {
                bail!("dial to {} was cancelled", peer.agent_id);
            }
            guard = tokio::time::timeout(
                super::super::connection::remaining_budget(deadline, "per-peer dial lock")
                    .map_err(|err| anyhow!("dial to {} failed: {err}", peer.agent_id))?,
                peer_lock.lock(),
            ) => {
                guard.map_err(|_| {
                    anyhow!(
                        "timed out waiting for the per-peer dial lock for {}",
                        peer.agent_id
                    )
                })?
            }
        };
        if let Some(existing) = self.registry.live_slot(&peer.agent_id).await {
            return Ok(existing);
        }
        let permit = self
            .connection_semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow!("connection limit reached"))?;
        let connecting = self
            .endpoint
            .connect(peer.addr, peer.agent_id.as_str())
            .with_context(|| format!("failed to begin QUIC connect to {}", peer.addr))?;
        let remote_addr = connecting.remote_address();
        // The handshake is the only long await before slot installation; it
        // is bounded by the REMAINING exchange budget (recomputed after the
        // lock wait) so a stalled dial cannot outlive the caller's deadline,
        // and cancellation-aware so revocation tears it down immediately.
        let connection = tokio::select! {
            _ = dial_cancel.cancelled() => {
                bail!("dial to {} was cancelled", peer.agent_id);
            }
            result = tokio::time::timeout(
                super::super::connection::remaining_budget(deadline, "QUIC handshake")
                    .map_err(|err| anyhow!("dial to {} failed: {err}", peer.agent_id))?,
                with_handshake_remote_addr(remote_addr, connecting),
            ) => result
                .map_err(|_| anyhow!("QUIC handshake timed out with {}", peer.addr))?
                .with_context(|| format!("QUIC handshake failed with {}", peer.addr))?,
        };
        let authenticated = AgentId::parse(&derive_peer_id_from_connection(&connection)?)?;
        if authenticated != peer.agent_id {
            connection.close(0u32.into(), b"authenticated peer mismatch");
            bail!(
                "authenticated peer {authenticated} does not match {}",
                peer.agent_id
            );
        }
        // Revocation race guard: the handshake may have started before the
        // peer was revoked. Two gates run inside the registry's admission
        // lock (see `admit_gated`): unchanged per-peer enrollment epoch AND
        // current enrollment in the published pins. The enrollment half
        // linearizes against the pin publication that completes
        // `remove_peer`; the epoch half against `close_peer`'s epoch bump.
        // Either the gates observe the revocation, or the subsequent
        // `close_peer` tears the fresh slot down. A pre-revocation attempt
        // is never admitted against restored trust; it must re-dial. The
        // gate is synchronous, so it cannot stall the registry lock.
        let gate_agent_id = peer.agent_id.clone();
        match self
            .registry
            .admit_gated(
                peer.agent_id.clone(),
                connection.clone(),
                Direction::Outbound,
                self.admission_gate(gate_agent_id, epoch_at_dial_start),
            )
            .await
        {
            Admission::Accepted { generation } => {
                self.spawn_connection_loop(
                    peer.agent_id.clone(),
                    generation,
                    connection.clone(),
                    permit,
                );
                Ok(connection)
            }
            Admission::Existing(existing) => Ok(existing),
            Admission::Rejected => bail!(
                "peer {} was revoked or admission could not be verified \
                 during connection setup",
                peer.agent_id
            ),
        }
    }
}
