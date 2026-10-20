//! The published pinning snapshot: the read-only trust view consumed by the
//! TLS verifiers and the connection-admission gate.
//!
//! Split from `mod.rs` for file-length limits.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock as StdRwLock};

/// Immutable pin map: `agent_id -> base64 public key`.
pub type PinningSnapshot = Arc<BTreeMap<String, String>>;

/// Shared handle to the published pinning snapshot.
///
/// Long-lived consumers (the TLS verifiers, the connection-admission gate)
/// capture a handle once and call [`read`](Self::read) on every use; the
/// directory replaces the snapshot's contents in place (see
/// [`crate::peer_directory::PeerDirectory`]), so a captured handle observes
/// later enrollment changes. The handle exposes reads only — publishing pins
/// is crate-private, so a library consumer holding this value cannot mint
/// trust that no enrollment authorized.
///
/// ```compile_fail,E0624
/// use axon::peer_directory::PinningSnapshotHandle;
/// fn probe(handle: PinningSnapshotHandle) {
///     // `replace` is crate-private: external callers cannot publish pins.
///     handle.replace(std::sync::Arc::new(std::collections::BTreeMap::new()));
/// }
/// ```
#[derive(Clone, Debug)]
pub struct PinningSnapshotHandle(Arc<StdRwLock<PinningSnapshot>>);

impl PinningSnapshotHandle {
    /// Wrap a snapshot in a standalone handle.
    ///
    /// Production publication goes through [`PeerDirectory`] (whose writer is
    /// crate-private); this constructor exists for consumers that need a
    /// self-contained pin table, e.g. transport tests injecting pins directly
    /// into a verifier.
    ///
    /// [`PeerDirectory`]: crate::peer_directory::PeerDirectory
    pub fn from_snapshot(snapshot: PinningSnapshot) -> Self {
        Self(Arc::new(StdRwLock::new(snapshot)))
    }

    /// Read the current snapshot. Lock poisoning is surfaced to the caller;
    /// the TLS verifiers map it to a handshake failure.
    pub fn read(&self) -> std::sync::LockResult<std::sync::RwLockReadGuard<'_, PinningSnapshot>> {
        self.0.read()
    }

    /// Replace the published snapshot in place. Crate-private by design: only
    /// `PeerDirectory` may change trust pins.
    ///
    /// Poisoning is recovered rather than propagated: the write is a single
    /// assignment that cannot leave the map torn, and losing publication
    /// would strand every captured consumer on a stale trust view.
    pub(crate) fn replace(&self, snapshot: PinningSnapshot) {
        match self.0.write() {
            Ok(mut guard) => *guard = snapshot,
            Err(poisoned) => *poisoned.into_inner() = snapshot,
        }
    }
}
