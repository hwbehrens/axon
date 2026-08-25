//! Outbound dial path for [`ConnectionManager`]: authoritative slot lookup,
//! bounded handshakes, and per-peer dial cancellation.
//!
//! Split from `quic_transport.rs` for file-length limits. This is a child
//! module, so the inherent `impl` below retains access to the manager's
//! private fields.

use std::sync::Arc;
use std::time::{Duration, Instant};

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
        if let Some(existing) = self.registry.current(peer).await {
            return Ok(existing);
        }
        let dial_cancel = self.dial_token(peer).await;
        let targets = directory.dial_targets(peer).await;
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
                    match tokio::time::timeout(resolve_budget, locator.resolve()).await {
                        Ok(Ok(resolved)) => addresses.extend(resolved),
                        Ok(Err(err)) => last_error = Some(SendError::pre_send(err)),
                        Err(_) => {
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
            let Some(budget) = deadline.checked_duration_since(Instant::now()) else {
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
                    budget.min(DIAL_TIMEOUT),
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
        self.dial(peer, DIAL_TIMEOUT).await
    }

    async fn dial(&self, peer: &DialPeer, handshake_budget: Duration) -> Result<quinn::Connection> {
        if let Some(existing) = self.registry.current(&peer.agent_id).await {
            return Ok(existing);
        }
        let dial_cancel = self.dial_token(&peer.agent_id).await;
        let peer_lock = {
            let mut locks = self.connecting_locks.lock().await;
            locks
                .entry(peer.agent_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = peer_lock.lock().await;
        if let Some(existing) = self.registry.current(&peer.agent_id).await {
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
        // is bounded so a stalled dial cannot outlive the caller's budget,
        // and cancellation-aware so revocation tears it down immediately.
        let connection = tokio::select! {
            _ = dial_cancel.cancelled() => {
                bail!("dial to {} was cancelled", peer.agent_id);
            }
            result = tokio::time::timeout(
                handshake_budget,
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
        // peer was revoked. The enrollment gate runs under the registry's
        // admission lock (see `admit_gated`), linearizing it against
        // `remove_peer`: either the gate sees the revocation, or the
        // subsequent `close_peer` tears the fresh slot down.
        match self
            .registry
            .admit_gated(
                peer.agent_id.clone(),
                connection.clone(),
                Direction::Outbound,
                || self.directory.is_enrolled(&peer.agent_id),
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
            Admission::Rejected => {
                bail!("peer {} was revoked during connection setup", peer.agent_id)
            }
        }
    }
}
