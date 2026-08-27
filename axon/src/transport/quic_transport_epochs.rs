//! Per-peer enrollment epochs and the deadline-bounded admission gate for
//! [`ConnectionManager`].
//!
//! Split from `quic_transport.rs` for file-length limits. This is a child
//! module, so the inherent `impl` below retains access to the manager's
//! private fields.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use crate::message::AgentId;

use super::ADMISSION_GATE_BUDGET;
use super::ConnectionManager;

impl ConnectionManager {
    /// The dialed peer's current enrollment epoch. Missing peers have never
    /// been revoked and start at zero.
    pub(super) fn peer_enrollment_epoch(&self, peer: &AgentId) -> u64 {
        self.enrollment_epochs
            .lock()
            .expect("enrollment epoch lock")
            .get(peer)
            .map(|epoch| epoch.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Snapshot of every tracked peer's enrollment epoch, taken BEFORE an
    /// inbound handshake begins. At admission time only the ACTUAL peer's
    /// entry is compared, so revoking one peer never invalidates another
    /// peer's in-flight handshake while still rejecting handshakes that
    /// started before their own peer's revocation committed.
    pub(super) fn capture_enrollment_epochs(&self) -> HashMap<AgentId, u64> {
        self.enrollment_epochs
            .lock()
            .expect("enrollment epoch lock")
            .iter()
            .map(|(peer, epoch)| (peer.clone(), epoch.load(Ordering::Relaxed)))
            .collect()
    }

    pub(super) fn advance_enrollment_epoch(&self, peer: &AgentId) {
        self.enrollment_epochs
            .lock()
            .expect("enrollment epoch lock")
            .entry(peer.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Build the two-part admission gate: the peer's enrollment epoch must be
    /// unchanged since `captured_epoch`, and the peer must currently be
    /// enrolled. The enrollment lookup is deadline-bounded (`deadline`, or
    /// [`ADMISSION_GATE_BUDGET`] for inbound handshakes without a caller
    /// deadline) and fails CLOSED: a stalled lookup rejects the connection
    /// rather than admitting it unexamined or blocking the registry lock
    /// forever.
    pub(super) fn admission_gate(
        &self,
        peer: AgentId,
        captured_epoch: u64,
        deadline: Option<Instant>,
    ) -> impl Future<Output = bool> + Send + 'static {
        let directory = self.directory.clone();
        let epochs = self.enrollment_epochs.clone();
        async move {
            let current = epochs
                .lock()
                .expect("enrollment epoch lock")
                .get(&peer)
                .map(|epoch| epoch.load(Ordering::Relaxed))
                .unwrap_or(0);
            if current != captured_epoch {
                return false;
            }
            let budget = match deadline {
                Some(deadline) => deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or_default(),
                None => ADMISSION_GATE_BUDGET,
            };
            tokio::time::timeout(budget, directory.is_enrolled(&peer)).await == Ok(true)
        }
    }
}
