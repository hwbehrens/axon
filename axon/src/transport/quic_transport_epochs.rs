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

use tracing::warn;

use crate::message::AgentId;

use super::ConnectionManager;

impl ConnectionManager {
    /// The dialed peer's current enrollment epoch. Missing peers have never
    /// been revoked and start at zero.
    ///
    /// A poisoned epoch lock reads as `0` (the never-revoked default). This
    /// value is advisory only: the admission gate reads the SAME lock and
    /// fails closed while it is poisoned, so a capture taken during
    /// poisoning can never be admitted.
    pub(super) fn peer_enrollment_epoch(&self, peer: &AgentId) -> u64 {
        let Ok(epochs) = self.enrollment_epochs.lock() else {
            return 0;
        };
        epochs
            .get(peer)
            .map(|epoch| epoch.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Snapshot of every tracked peer's enrollment epoch, taken BEFORE an
    /// inbound handshake begins. At admission time only the ACTUAL peer's
    /// entry is compared, so revoking one peer never invalidates another
    /// peer's in-flight handshake while still rejecting handshakes that
    /// started before their own peer's revocation committed.
    ///
    /// A poisoned epoch lock yields an empty snapshot (every capture reads
    /// as zero). Safe for the same reason as [`Self::peer_enrollment_epoch`]:
    /// the admission gate fails closed while the lock is poisoned.
    pub(super) fn capture_enrollment_epochs(&self) -> HashMap<AgentId, u64> {
        let Ok(epochs) = self.enrollment_epochs.lock() else {
            return HashMap::new();
        };
        epochs
            .iter()
            .map(|(peer, epoch)| (peer.clone(), epoch.load(Ordering::Relaxed)))
            .collect()
    }

    pub(super) fn advance_enrollment_epoch(&self, peer: &AgentId) {
        match self.enrollment_epochs.lock() {
            Ok(mut epochs) => {
                epochs
                    .entry(peer.clone())
                    .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(_poisoned) => {
                // A poisoned lock means a previous holder panicked
                // mid-operation; we never recover it, so this bump is
                // skipped until restart. Revocation stays safe: the
                // admission gate fails closed while the lock is poisoned
                // (no stale attempt can be admitted against any epoch), and
                // the slot-teardown half of the revocation guarantee runs in
                // `close_peer` regardless.
                warn!(
                    peer = %peer,
                    "enrollment epoch lock poisoned; skipping epoch bump \
                     (admission gate fails closed while poisoned)"
                );
            }
        }
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
    /// - a just-revoked peer may be briefly admitted, but
    ///   [`ConnectionManager::revoke_peer`] always follows the directory
    ///   commit with `close_peer`, which removes the freshly installed slot.
    ///   The documented revocation guarantee is unchanged: either the gate
    ///   refuses, or the revocation itself tears the slot down.
    ///
    /// Poisoning fails CLOSED: a poisoned epoch lock (a previous holder
    /// panicked mid-operation) rejects every admission until restart rather
    /// than guessing whether the peer's epoch moved. Poisoning is permanent —
    /// the lock is never recovered or cleared.
    pub(super) fn admission_gate(&self, peer: AgentId, captured_epoch: u64) -> impl Fn() -> bool {
        let pins = self.directory.pinning_snapshot();
        let epochs = self.enrollment_epochs.clone();
        move || {
            let Ok(epochs) = epochs.lock() else {
                return false;
            };
            let current = epochs
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
