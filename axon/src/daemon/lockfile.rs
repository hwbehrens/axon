use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

/// Maximum acquisition attempts. Exists only so a pathological
/// replace-the-lock-path race terminates; real acquisition succeeds on the
/// first attempt.
const MAX_ACQUIRE_ATTEMPTS: usize = 8;

/// Single-instance daemon lock held for the daemon's lifetime.
///
/// Mutual exclusion is an OS-level `flock(2)` on the lock file, not the file
/// contents: the file's PID line is a diagnostic publication, while the
/// kernel lock is the authority. A crashed daemon's lock is released by the
/// kernel when its descriptor closes, so stale or malformed files never
/// block startup, and PID reuse cannot lock a healthy daemon out.
pub(crate) struct DaemonLock {
    path: PathBuf,
    file: File,
    released: bool,
}

impl DaemonLock {
    pub(crate) fn acquire(lock_path: &Path) -> Result<Self> {
        let path = lock_path.to_path_buf();

        for _ in 0..MAX_ACQUIRE_ATTEMPTS {
            reject_non_regular_lock_path(&path)?;

            // Never truncate before the lock is ours: an established
            // holder's published PID must survive a failed acquisition.
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                // A symlink swapped in after the type check above must not be
                // followed.
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .with_context(|| format!("failed to open daemon lock file: {}", path.display()))?;

            match try_lock_exclusive(&file) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    // The flock — not the PID line — proves a live holder.
                    let holder = read_lock_pid(&path)
                        .map(|pid| format!(" (pid {pid})"))
                        .unwrap_or_default();
                    bail!("daemon already running{holder} on this state root");
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to lock daemon lock file: {}", path.display())
                    });
                }
            }

            // The type check ran before open, but the path could have been
            // swapped: verify the inode the descriptor actually locked.
            let fd_meta = file.metadata().with_context(|| {
                format!("failed to inspect daemon lock file: {}", path.display())
            })?;
            if !fd_meta.file_type().is_file() {
                bail!(
                    "refusing to use non-regular daemon lock file: {}",
                    path.display()
                );
            }

            // The lock is ours, but only if the path still names the inode we
            // locked: a previous holder may have unlinked it between our open
            // and flock, leaving us holding an orphaned inode while the path
            // is free for a fresh daemon to create. Retry on mismatch.
            if !path_refers_to_same_file(&file, &path) {
                drop(file);
                continue;
            }

            publish_pid(&file, &path)?;
            info!(pid = std::process::id(), path = %path.display(), "acquired daemon lock");
            return Ok(Self {
                path,
                file,
                released: false,
            });
        }

        bail!(
            "daemon lock path changed repeatedly during acquisition: {}",
            path.display()
        );
    }

    pub(crate) fn release(&mut self) -> Result<()> {
        self.release_inner()
            .with_context(|| format!("failed to remove daemon lock file: {}", self.path.display()))
    }

    fn release_inner(&mut self) -> std::io::Result<()> {
        if self.released {
            return Ok(());
        }
        // Unlink only while still holding the flock, and only when the path
        // still names the inode this process locked: if the file was removed
        // and recreated by someone else, removing it would delete a lock we
        // do not own.
        if path_refers_to_same_file(&self.file, &self.path) {
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        self.released = true;
        Ok(())
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        if let Err(err) = self.release_inner() {
            warn!(
                error = %err,
                path = %self.path.display(),
                "failed to remove daemon lock file"
            );
        }
    }
}

fn reject_non_regular_lock_path(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => {
            bail!(
                "refusing to use non-regular daemon lock file: {}",
                path.display()
            );
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err)
            .with_context(|| format!("failed to inspect daemon lock file: {}", path.display())),
    }
}

fn try_lock_exclusive(file: &File) -> std::io::Result<()> {
    // SAFETY: flock(2) operates only on the descriptor owned by `file`;
    // LOCK_NB keeps it non-blocking. The kernel releases the lock when the
    // descriptor closes, so a crashed daemon cannot strand it.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    Err(std::io::Error::last_os_error())
}

fn path_refers_to_same_file(file: &File, path: &Path) -> bool {
    let Ok(fd_meta) = file.metadata() else {
        return false;
    };
    let Ok(path_meta) = fs::symlink_metadata(path) else {
        return false;
    };
    fd_meta.dev() == path_meta.dev() && fd_meta.ino() == path_meta.ino()
}

fn publish_pid(file: &File, path: &Path) -> Result<()> {
    // Fixed-width record published with one positioned write, so a reader
    // never sees an empty file — at worst the previous holder's line while
    // the lock is held. The flock, never the content, is the
    // mutual-exclusion authority, so the worst case is an unhelpful
    // "already running" message for a loser.
    let pid_bytes = format!("{:<11}\n", std::process::id()).into_bytes();
    file.write_at(&pid_bytes, 0).with_context(|| {
        format!(
            "failed to publish PID in daemon lock file: {}",
            path.display()
        )
    })?;
    file.set_len(pid_bytes.len() as u64)
        .with_context(|| format!("failed to truncate daemon lock file: {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync daemon lock file: {}", path.display()))
}

fn read_lock_pid(path: &Path) -> Option<u32> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) => {
            warn!(
                error = %err,
                path = %path.display(),
                "failed reading daemon lock file"
            );
            return None;
        }
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    match trimmed.parse::<u32>() {
        Ok(pid) => Some(pid),
        Err(err) => {
            warn!(
                error = %err,
                path = %path.display(),
                "daemon lock file contains invalid PID"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "lockfile_tests.rs"]
mod tests;
