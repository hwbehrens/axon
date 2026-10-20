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

/// Maximum private recovery names tried while releasing the lock.
const MAX_RELEASE_CAPTURE_ATTEMPTS: usize = 8;

/// Single-instance daemon lock held for the daemon's lifetime.
///
/// Mutual exclusion is an OS-level `flock(2)` on the lock file, not the file
/// contents: the file's PID line is a diagnostic publication, while the
/// kernel lock is the authority. A crashed daemon's lock is released by the
/// kernel when its descriptor closes, so a dead or malformed PID record never
/// blocks startup.
///
/// Transitional backstop: a PID record naming a live foreign process is
/// refused even after this process wins the flock, because a pre-flock AXON
/// daemon (or a daemon whose lock file was replaced) may still be running
/// against the same state root. The trade-off is fail-closed by design: if a
/// crash leaves such a record and an unrelated process later recycles that
/// PID, startup keeps failing until the record is removed; the error names
/// the file and the removal guidance, and `axon doctor` reports the same
/// state as manual-only instead of deleting the guard itself (DEC-028). PID
/// reuse may therefore refuse startup, never admit a second daemon.
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

            // Transitional backstop for pre-flock daemons: older versions
            // published their PID here but held no flock, so winning the
            // flock does not prove the recorded daemon is gone. Fail closed
            // while the file names a live process that is not this one; a
            // genuinely stale record (dead, or recycled to a dead PID) is
            // still replaced as before. The flock remains the authority.
            if let Some(pid) = read_lock_pid(&path)
                && pid != std::process::id()
                && pid_is_alive(pid)
            {
                bail!(
                    "daemon.pid names live process {pid} that does not hold the daemon lock; \
                     refusing to start (remove {} only if that process is not an AXON daemon)",
                    path.display()
                );
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
        // The flock is still held here, but the path can be swapped between
        // an ownership check and an unlink (a cleanup script or a successor
        // racing the release), which could delete a lock file this process
        // does not own. So capture the current entry under a private name
        // first — the rename is atomic — decide on the captured inode, and
        // only ever unlink that private name. A captured successor file is
        // put back under its original path instead of being severed.
        let Some(captured) = self.capture_entry()? else {
            self.released = true;
            return Ok(());
        };
        if path_refers_to_same_file(&self.file, &captured) {
            remove_file_if_present(&captured)?;
        } else {
            self.restore_successor(&captured)?;
        }
        self.released = true;
        Ok(())
    }

    /// Atomically move whatever the lock path currently names to a private
    /// recovery name and return it; `None` when the path is already gone.
    fn capture_entry(&self) -> std::io::Result<Option<PathBuf>> {
        let stem = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "daemon.pid".to_string());
        for attempt in 0..MAX_RELEASE_CAPTURE_ATTEMPTS {
            let captured = self
                .path
                .with_file_name(format!("{stem}.released.{}.{attempt}", std::process::id()));
            // Reserve the recovery name first: `rename` would otherwise
            // silently replace (delete) a file already sitting there.
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&captured)
            {
                Ok(_sentinel) => {}
                Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
            #[cfg(test)]
            release_hook::run();
            match fs::rename(&self.path, &captured) {
                Ok(()) => return Ok(Some(captured)),
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    let _ = remove_file_if_present(&captured);
                    return Ok(None);
                }
                Err(err) => {
                    let _ = remove_file_if_present(&captured);
                    return Err(err);
                }
            }
        }
        Err(std::io::Error::new(
            ErrorKind::AlreadyExists,
            "could not reserve a private name to release the daemon lock",
        ))
    }

    /// Put a captured successor lock file back under the original path
    /// without clobbering anything created there in the meantime.
    fn restore_successor(&self, captured: &Path) -> std::io::Result<()> {
        match fs::hard_link(captured, &self.path) {
            Ok(()) => remove_file_if_present(captured),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                warn!(
                    preserved = %captured.display(),
                    "daemon lock path was replaced during release; the displaced file was preserved"
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    // SAFETY: kill(pid, 0) sends no signal; it only probes process existence
    // and permission to signal.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0
        || matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(code) if code == libc::EPERM
        )
}

/// One-shot test injection point inside `capture_entry`: it runs after the
/// private recovery name is reserved but before the lock entry is captured,
/// letting a test replace the path exactly in the window between the
/// ownership check and the unlink the old implementation performed.
#[cfg(test)]
mod release_hook {
    use std::cell::RefCell;

    thread_local! {
        static HOOK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    pub(super) fn install(hook: impl FnOnce() + 'static) {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    pub(super) fn run() {
        if let Some(hook) = HOOK.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
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
