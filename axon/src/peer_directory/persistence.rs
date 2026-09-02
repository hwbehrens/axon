//! Transactional persistence for [`PeerDirectory`]'s persistent edits.
//!
//! Split from `mod.rs` for file-length limits. This is a child module, so
//! the inherent `impl` below retains access to the directory's private
//! fields.
//!
//! Invariants (DEC-021, DEC-022, DEC-023):
//!
//! - **No peer-store disk I/O under the state lock** (read or write): a
//!   stalled save must never block readers (`dial_targets`, the transport
//!   admission gate) or writers (`observe`). The save runs between the
//!   read-lock snapshot and the write-lock apply, guarded only by the save
//!   gate.
//! - **One fully serialized transaction per edit**: the save gate is held
//!   across build, save, AND apply. Because every mutation of the persistent
//!   set (`enrolled`) happens inside this serialized section, the state
//!   observed at build time is still current at apply time. There are no
//!   lost races to retry, no speculative saves to heal, and no interleaving
//!   — including caller cancellation, which is shielded by the owned
//!   transaction worker — that can leave disk and memory divergent.
//!   (Rounds six and seven used save-then-apply with a generation retry
//!   loop and a heal path; both carried windows where disk could end up
//!   older than memory, and the retry budget failed under contention. The
//!   retry loop, heal path, and the defensive generation tripwire they
//!   required were all removed once transactions became fully serialized.)
//! - **`store.save` never errors after its rename**: post-rename failures
//!   are durability warnings, so an `Err` always means the file is
//!   unchanged and the edit must not be applied to memory.

use anyhow::anyhow;
use tokio::task::JoinHandle;
use tracing::debug;

use super::state::DirectoryState;
use super::types::DirectoryError;
use super::{PeerDirectory, PinningSnapshot};

/// A validated persistent edit, built against a read snapshot of directory
/// state.
///
/// The plan's `saved_state` snapshot is what gets serialized; `apply` puts
/// the same delta onto live state (never a whole-snapshot swap, which would
/// clobber concurrent ephemeral changes such as new observations).
pub(super) struct PersistPlan<T> {
    /// Snapshot whose persistent content (`stored_peers`) equals post-apply
    /// state. Serialized to the peer store inside the transaction.
    pub(super) saved_state: DirectoryState,
    /// Applies this edit's delta onto current live state under the
    /// transaction's write lock.
    pub(super) apply: Box<dyn FnOnce(&mut DirectoryState) + Send>,
    /// Value returned to the caller on successful commit (or on validation
    /// fast paths such as re-enrolling an already-enrolled peer).
    pub(super) value: T,
}

impl PeerDirectory {
    /// Commit a persistent edit through the owned transaction worker.
    ///
    /// Awaits only the worker's join handle: if the caller is dropped at
    /// this await, the worker still completes save-plus-apply, so disk and
    /// memory can never diverge because of a cancelled request.
    pub(super) async fn commit_persistent<T>(
        &self,
        build: impl FnOnce(&DirectoryState) -> Result<PersistPlan<T>, DirectoryError> + Send + 'static,
    ) -> Result<T, DirectoryError>
    where
        T: Send + 'static,
    {
        self.spawn_persistent_edit(build).await.map_err(|err| {
            DirectoryError::Persist(anyhow!("peer-directory transaction worker failed: {err}"))
        })?
    }

    /// Spawn the transaction worker without awaiting it. Exposed separately
    /// so tests can prove a detached worker still commits.
    pub(super) fn spawn_persistent_edit<T>(
        &self,
        build: impl FnOnce(&DirectoryState) -> Result<PersistPlan<T>, DirectoryError> + Send + 'static,
    ) -> JoinHandle<Result<T, DirectoryError>>
    where
        T: Send + 'static,
    {
        let directory = self.clone();
        tokio::spawn(async move {
            let result = directory.run_persistent_edit(build).await;
            if let Err(err) = &result {
                debug!(error = %err, "peer-directory persistent edit did not commit");
            }
            result
        })
    }

    /// Run one persistent edit as a fully serialized transaction.
    ///
    /// The save gate is acquired FIRST and held across build, save, and
    /// apply. Every mutation of the persistent set (`enrolled`) happens
    /// under this gate, so the state built from the snapshot is still
    /// current when the delta is applied — no retry loop exists to fail, and
    /// no generation counter is needed to detect a race.
    ///
    /// Lock ordering is strictly gate -> state lock; no path takes a state
    /// lock and then the gate, so no deadlock is possible. Disk I/O runs
    /// between the read-lock snapshot and the write-lock apply: never under
    /// the state lock.
    async fn run_persistent_edit<T>(
        &self,
        build: impl FnOnce(&DirectoryState) -> Result<PersistPlan<T>, DirectoryError>,
    ) -> Result<T, DirectoryError> {
        // The guard is held (by binding) to the end of the transaction:
        // build, save, apply all run under the gate.
        let _gate = self.save_lock.lock().await;
        let plan = {
            let state = self.state.read().await;
            build(&state)?
        };
        self.store
            .save(plan.saved_state.stored_peers())
            .await
            .map_err(DirectoryError::Persist)?;
        let mut state = self.state.write().await;
        (plan.apply)(&mut state);
        let pins = state.pinning_snapshot();
        drop(state);
        self.publish_pins(pins);
        Ok(plan.value)
    }

    /// Publish the new snapshot by mutating the shared lock's contents in
    /// place. Consumers capture the handle once (TLS verifiers at bind, the
    /// admission gate per outbound dial and per inbound handshake) and keep
    /// reading it forever, so swapping the `pins` field with a new
    /// `Arc<StdRwLock<..>>` would strand them on a frozen snapshot. See the
    /// field doc on `PeerDirectory::pins`.
    fn publish_pins(&self, pins: PinningSnapshot) {
        match self.pins.write() {
            Ok(mut guard) => *guard = pins,
            Err(poisoned) => *poisoned.into_inner() = pins,
        }
    }
}
