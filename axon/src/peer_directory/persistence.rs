//! Lock-free, cancellation-safe persistence for [`PeerDirectory`]'s
//! persistent edits.
//!
//! Split from `mod.rs` for file-length limits. This is a child module, so
//! the inherent `impl` below retains access to the directory's private
//! fields.
//!
//! Invariants (DEC-021, DEC-022):
//!
//! - **No peer-store disk I/O under the state lock** (read or write): a
//!   stalled save must never block readers (`dial_targets`, the transport
//!   admission gate) or writers (`observe`). Snapshot under a read lock,
//!   save with no state lock, apply under a short write lock.
//! - **save-then-apply is shielded from caller cancellation**: the whole
//!   retry loop runs on an owned transaction worker, so dropping the
//!   caller's future (timeout, shutdown race) can never leave disk ahead of
//!   memory.
//! - **`store.save` never errors after its rename**: post-rename failures
//!   are durability warnings, so an `Err` always means the file is
//!   unchanged and the edit must not be applied to memory.

use anyhow::anyhow;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::state::DirectoryState;
use super::store::StoredPeer;
use super::types::DirectoryError;
use super::{PeerDirectory, PinningSnapshot};

/// Maximum attempts for a persistent edit that keeps losing the
/// persist-generation race before the store is healed from live memory and
/// an error is returned.
pub(super) const PERSIST_COMMIT_ATTEMPTS: usize = 8;

/// A validated persistent edit, built against a read snapshot of directory
/// state.
///
/// Peer-store disk I/O must not run under the state write lock: a stalled
/// save would block every reader (`dial_targets`, `is_enrolled`) — including
/// the transport's send path and connection-admission gate — indefinitely.
/// Instead an edit is validated against a snapshot, its bytes are saved with
/// no lock held, and then a short write lock applies the same delta onto
/// fresh live state (never a whole-snapshot swap, which would clobber
/// concurrent ephemeral changes such as new observations).
pub(super) struct PersistPlan<T> {
    /// Snapshot whose persistent content (`stored_peers`) equals post-apply
    /// state when the persist-generation check passes. This is what gets
    /// serialized.
    pub(super) saved_state: DirectoryState,
    /// Applies this edit's delta onto current live state under the short
    /// commit lock.
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
        build: impl Fn(&DirectoryState) -> Result<PersistPlan<T>, DirectoryError> + Send + 'static,
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
        build: impl Fn(&DirectoryState) -> Result<PersistPlan<T>, DirectoryError> + Send + 'static,
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

    async fn run_persistent_edit<T>(
        &self,
        build: impl Fn(&DirectoryState) -> Result<PersistPlan<T>, DirectoryError>,
    ) -> Result<T, DirectoryError> {
        for _attempt in 0..PERSIST_COMMIT_ATTEMPTS {
            let (plan, generation) = {
                let state = self.state.read().await;
                (build(&state)?, state.persist_generation)
            };
            self.save_serialized(plan.saved_state.stored_peers())
                .await?;
            let mut state = self.state.write().await;
            if state.persist_generation != generation {
                debug!("peer-directory edit lost a persist race; retrying");
                continue;
            }
            (plan.apply)(&mut state);
            state.persist_generation += 1;
            let pins = state.pinning_snapshot();
            drop(state);
            self.publish_pins(pins);
            return Ok(plan.value);
        }
        // Lost every race: heal the store from live memory (memory is the
        // authority), then surface a retryable error.
        self.heal_store().await?;
        Err(DirectoryError::Persist(anyhow!(
            "peer directory changed concurrently during persistence; \
             edit abandoned after {PERSIST_COMMIT_ATTEMPTS} attempts"
        )))
    }

    /// Serialize every peer-store write. Saves are individually atomic
    /// (temp file + rename), but serializing them keeps a losing racer's
    /// speculative bytes from landing after a newer committed save.
    async fn save_serialized(&self, peers: Vec<StoredPeer>) -> Result<(), DirectoryError> {
        let _serialized = self.save_lock.lock().await;
        self.store
            .save(peers)
            .await
            .map_err(DirectoryError::Persist)
    }

    /// Re-persist live memory with NO state lock held across disk I/O: the
    /// snapshot is cloned under a read lock, saved while the save lock is
    /// held, and committed (generation-checked, no-op apply) under a short
    /// write lock.
    ///
    /// The generation re-check runs while HOLDING the save lock, so a
    /// committer that builds between snapshot and save cannot write its
    /// bytes before ours and apply after ours; the no-op generation bump in
    /// the commit invalidates any such committer's speculative save and
    /// forces a retry against fresh state.
    async fn heal_store(&self) -> Result<(), DirectoryError> {
        for _attempt in 0..PERSIST_COMMIT_ATTEMPTS {
            let (peers, generation) = {
                let state = self.state.read().await;
                (state.stored_peers(), state.persist_generation)
            };
            let save_guard = self.save_lock.lock().await;
            if self.state.read().await.persist_generation != generation {
                continue;
            }
            self.store
                .save(peers)
                .await
                .map_err(DirectoryError::Persist)?;
            drop(save_guard);
            let mut state = self.state.write().await;
            if state.persist_generation != generation {
                continue;
            }
            // No-op apply: bump the generation so any committer that saved
            // speculative bytes in the gap fails its check and retries (its
            // edit re-applies on fresh state).
            state.persist_generation += 1;
            return Ok(());
        }
        warn!("peer-store heal could not quiesce behind concurrent commits");
        Err(DirectoryError::Persist(anyhow!(
            "peer-store heal could not quiesce after {PERSIST_COMMIT_ATTEMPTS} attempts"
        )))
    }

    fn publish_pins(&self, pins: PinningSnapshot) {
        match self.pins.write() {
            Ok(mut guard) => *guard = pins,
            Err(poisoned) => *poisoned.into_inner() = pins,
        }
    }
}
