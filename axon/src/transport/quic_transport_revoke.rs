//! Revocation path for [`ConnectionManager`]: the single sanctioned
//! directory-commit-then-teardown pairing.
//!
//! Split from `quic_transport.rs` for file-length limits. This is a child
//! module, so the inherent `impl` below retains access to the manager's
//! private fields.

use anyhow::anyhow;

use crate::message::AgentId;
use crate::peer_directory::{DirectoryError, PeerIdentity};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use tokio::sync::Notify;

use super::ConnectionManager;

/// Test-only state behind [`ConnectionManager::revocation_pause`].
#[cfg(test)]
#[derive(Default)]
pub(super) struct RevocationPause {
    pub(super) armed: AtomicBool,
    pub(super) release: Notify,
}

impl ConnectionManager {
    /// Revoke a peer through the single sanctioned pairing: commit trust
    /// removal to the directory first, THEN tear down transport state. The
    /// admission gate's revocation guarantee — either the gate refuses a
    /// handshake that raced the revocation, or the subsequent `close_peer`
    /// tears the freshly installed slot down — requires `close_peer` to
    /// follow every successful directory commit. This method exists so the
    /// pairing cannot be skipped by a future caller: a bare
    /// `PeerDirectory::remove_peer` (crate-private for exactly this reason)
    /// without a paired `close_peer` could leave a just-admitted connection
    /// live against revoked trust.
    ///
    /// The pair runs on a task owned by the manager's task tracker and
    /// detached from this future's caller. The directory's persistent edit
    /// is itself shielded from cancellation (its transaction worker
    /// completes save-plus-apply once started), so a caller cancelled
    /// between commit and teardown would otherwise strand a live connection
    /// against revoked trust. Once this method's body has started, the pair
    /// completes regardless of what happens to the caller; tracker
    /// ownership means `close_all` joins in-flight revocations at shutdown
    /// instead of aborting them mid-pair. The pair deliberately does NOT
    /// observe shutdown cancellation — once the commit lands, teardown must
    /// follow.
    ///
    /// A revocation racing shutdown may commit durably after `close_all`'s
    /// wait has returned (the check-then-spawn window is not synchronized).
    /// This is safe by construction: `close_all` closes the endpoint before
    /// waiting, so no live connection exists to strand, the directory
    /// remains consistent (its commit is atomic once started), and a
    /// post-shutdown `close_peer` is a no-op on an empty registry. The
    /// pairing's obligation — no live connection under revoked trust — is
    /// defined against LIVE transport; after shutdown there is nothing to
    /// tear down. A `JoinError` (runtime shutdown or task panic) surfaces
    /// as [`DirectoryError::Persist`].
    ///
    /// On a failed commit (peer not enrolled, persistence error) nothing is
    /// torn down: transport authority follows trust, never leads it.
    pub async fn revoke_peer(&self, agent_id: &AgentId) -> Result<PeerIdentity, DirectoryError> {
        let manager = self.clone();
        let agent_id = agent_id.clone();
        self.tasks
            .spawn(async move {
                let identity = manager.directory.remove_peer(&agent_id).await?;
                #[cfg(test)]
                if manager.revocation_pause.armed.load(Ordering::Acquire) {
                    manager.revocation_pause.release.notified().await;
                }
                manager.close_peer(&agent_id, b"peer revoked").await;
                Ok(identity)
            })
            .await
            .map_err(|err| {
                DirectoryError::Persist(anyhow!("revocation task failed before teardown: {err}"))
            })?
    }
}
