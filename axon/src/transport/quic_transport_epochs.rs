//! Per-peer enrollment epochs and the synchronous admission gate for
//! [`ConnectionManager`].
//!
//! Split from `quic_transport.rs` for file-length limits. This is a child
//! module, so the inherent `impl` below retains access to the manager's
//! private fields.

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::message::AgentId;

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
    /// unchanged since `captured_epoch`, and the peer must currently appear
    /// in the published pinning snapshot. Both reads are synchronous (std
    /// locks), so the gate runs entirely inside the registry's write-lock
    /// critical section: nothing can hold the registry lock across an await,
    /// no stall can block admission, and no lock-ordering rule between the
    /// registry and the directory is required.
    ///
    /// The pin snapshot is the SAME immutable trust oracle the TLS verifiers
    /// consume. It is republished after every persistent enrollment commit,
    /// so it can lag live directory state only between a commit's apply and
    /// its pins publish. In that window:
    /// - a just-enrolled peer may be briefly refused (conservative; the
    ///   reconnect maintenance retries within one second), and
    /// - a just-revoked peer may be briefly admitted, but `remove_peer`'s
    ///   caller always follows with `close_peer`, which removes the freshly
    ///   installed slot. The documented revocation guarantee is unchanged:
    ///   either the gate refuses, or the revocation itself tears the slot
    ///   down.
    pub(super) fn admission_gate(&self, peer: AgentId, captured_epoch: u64) -> impl Fn() -> bool {
        let pins = self.directory.pinning_snapshot();
        let epochs = self.enrollment_epochs.clone();
        move || {
            let current = epochs
                .lock()
                .expect("enrollment epoch lock")
                .get(&peer)
                .map(|epoch| epoch.load(Ordering::Relaxed))
                .unwrap_or(0);
            if current != captured_epoch {
                return false;
            }
            pins.read()
                .map(|pins| pins.contains_key(peer.as_str()))
                .unwrap_or(false)
        }
    }
}
