//! Daemon artifact checks: the single-instance lock file and the IPC socket.
//!
//! The daemon's mutual exclusion is an OS-level `flock(2)` it holds for its
//! lifetime (axon/src/daemon/lockfile.rs), and the kernel releases it when
//! the process exits. Doctor therefore treats a *held* lock as the only
//! authoritative evidence that a daemon is running. The PID line inside the
//! file is a diagnostic publication: a live process that merely shares that
//! PID (recycled PID, unrelated holder) must never make doctor report a
//! running daemon, and malformed or dead content must never hide one.
//!
//! A lock file with no live holder is normally a stale leftover that fix mode
//! may remove, but one whose PID line names a live process is the exact state
//! the daemon's own startup backstop refuses to start beside (a pre-flock
//! daemon may still be running). Doctor reports it as manual-only and never
//! deletes the record or the socket that may belong to that daemon (DEC-028).

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::Path;

use anyhow::{Context, Result, bail};

use axon::config::AxonPaths;

use crate::app::doctor::{DoctorArgs, DoctorReport};

pub(in crate::app::doctor) fn check_daemon_artifacts(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    let lock_path = &paths.daemon_lock;
    let lock_state = inspect_daemon_lock(lock_path)?;
    let daemon_running = matches!(&lock_state, DaemonLockState::Held { .. });
    let live_backstop = matches!(&lock_state, DaemonLockState::LiveBackstop { .. });

    match lock_state {
        DaemonLockState::Missing => report.add_check(
            "daemon_pid",
            true,
            false,
            "daemon.pid not present".to_string(),
        ),
        DaemonLockState::Held { published } => {
            report.add_check("daemon_pid", true, false, held_message(&published));
        }
        DaemonLockState::NonRegular => report.add_check(
            "daemon_pid",
            false,
            false,
            format!(
                "daemon.pid is not a regular file: {} (manual cleanup required)",
                lock_path.display()
            ),
        ),
        DaemonLockState::LiveBackstop { pid } => report.add_check(
            "daemon_pid",
            false,
            false,
            format!(
                "daemon.pid names live process {pid} that does not hold the lock: \
                 manual removal required (remove {} only if that process is not an AXON daemon)",
                lock_path.display()
            ),
        ),
        DaemonLockState::Stale(stale) => {
            if args.fix {
                remove_stale_lock(lock_path, &stale)?;
                report.add_fix(
                    "daemon_pid_cleanup",
                    format!("removed stale daemon.pid ({})", stale.detail),
                );
                report.add_check(
                    "daemon_pid",
                    true,
                    true,
                    "removed stale daemon.pid".to_string(),
                );
            } else {
                report.add_check(
                    "daemon_pid",
                    false,
                    true,
                    format!(
                        "stale daemon.pid ({}): no process holds the lock",
                        stale.detail
                    ),
                );
            }
        }
    }

    if !paths.socket.exists() {
        if daemon_running {
            report.add_check(
                "ipc_socket",
                false,
                false,
                format!(
                    "daemon appears running but socket is missing: {}",
                    paths.socket.display()
                ),
            );
        } else {
            report.add_check(
                "ipc_socket",
                true,
                false,
                format!("socket not present ({})", paths.socket.display()),
            );
        }
        return Ok(());
    }

    let meta = fs::symlink_metadata(&paths.socket)
        .with_context(|| format!("failed to read socket metadata: {}", paths.socket.display()))?;
    if !meta.file_type().is_socket() {
        report.add_check(
            "ipc_socket",
            false,
            false,
            format!(
                "socket path exists but is not a unix socket: {} (manual cleanup required)",
                paths.socket.display()
            ),
        );
        return Ok(());
    }

    if daemon_running {
        report.add_check(
            "ipc_socket",
            true,
            false,
            format!("socket present ({})", paths.socket.display()),
        );
    } else if live_backstop {
        // The socket may belong to the pre-flock daemon the PID record
        // guards; deleting it could strand a live daemon or admit a second
        // one, exactly what the daemon's backstop exists to prevent.
        report.add_check(
            "ipc_socket",
            false,
            false,
            format!(
                "socket present but daemon.pid names a live process that does not hold the lock: \
                 manual cleanup required ({})",
                paths.socket.display()
            ),
        );
    } else if args.fix {
        fs::remove_file(&paths.socket)
            .with_context(|| format!("failed removing stale socket: {}", paths.socket.display()))?;
        report.add_fix(
            "ipc_socket_cleanup",
            format!("removed stale socket {}", paths.socket.display()),
        );
        report.add_check("ipc_socket", true, true, "removed stale socket".to_string());
    } else {
        report.add_check(
            "ipc_socket",
            false,
            true,
            format!(
                "socket exists but no daemon holds the lock: {}",
                paths.socket.display()
            ),
        );
    }

    Ok(())
}

/// What the lock file currently proves about a running daemon.
enum DaemonLockState {
    Missing,
    /// A process holds the `flock`: authoritative evidence of a live daemon.
    Held {
        published: PidLine,
    },
    /// Nobody holds the `flock` and the PID line does not name a live
    /// process: the file is a leftover artifact fix mode may remove.
    Stale(StaleLock),
    /// Nobody holds the `flock`, but the PID line names a live process the
    /// daemon's own startup backstop refuses to start beside. Removing the
    /// record could admit a second daemon while a pre-flock daemon still
    /// runs, so doctor reports it as manual-only and never deletes it.
    LiveBackstop {
        pid: u32,
    },
    /// The path is not a regular file; doctor never deletes such a path.
    NonRegular,
}

/// A stale lock file whose `flock` this process holds. Keeping the
/// descriptor open until the fix completes proves no daemon can acquire the
/// lock between the probe and the unlink, mirroring the daemon's own
/// release-while-locked discipline.
struct StaleLock {
    file: File,
    detail: String,
}

/// The diagnostic PID line published inside the lock file. Never liveness
/// evidence on its own, but a live value decides fix-mode classification:
/// the daemon's startup backstop treats such a record as a possible
/// pre-flock daemon, so doctor reports it as manual-only.
enum PidLine {
    Missing,
    Invalid(String),
    Valid(u32),
}

impl PidLine {
    fn detail(&self) -> String {
        match self {
            Self::Missing => "empty pid line".to_string(),
            Self::Invalid(raw) => format!("invalid pid value '{raw}'"),
            Self::Valid(pid) => format!("pid {pid}"),
        }
    }
}

fn inspect_daemon_lock(path: &Path) -> Result<DaemonLockState> {
    // Inspect before opening: opening a FIFO or device that squats on the
    // lock path could block or have side effects. The daemon refuses such
    // paths too (lockfile.rs), so doctor reports rather than touches them.
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => return Ok(DaemonLockState::NonRegular),
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(DaemonLockState::Missing),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }

    // O_NOFOLLOW: a symlink swapped in after the check above must not be
    // followed (the daemon opens its lock the same way).
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(DaemonLockState::Missing),
        Err(err) if err.raw_os_error() == Some(libc::ELOOP) => {
            return Ok(DaemonLockState::NonRegular);
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to open {}", path.display()));
        }
    };

    let meta = file
        .metadata()
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if !meta.file_type().is_file() {
        return Ok(DaemonLockState::NonRegular);
    }

    match try_lock_exclusive(&file) {
        Ok(()) => {
            // The lock is ours, so no daemon holds it. Mirror the daemon's
            // startup backstop: a record naming a live process may be
            // guarding a still-running pre-flock daemon, so it is
            // manual-only. Otherwise keep the lock held until the fix (or
            // the end of the check) releases it.
            match read_pid_line(&file)? {
                PidLine::Valid(pid) if pid_is_alive(pid) => {
                    Ok(DaemonLockState::LiveBackstop { pid })
                }
                published => Ok(DaemonLockState::Stale(StaleLock {
                    file,
                    detail: published.detail(),
                })),
            }
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => Ok(DaemonLockState::Held {
            published: read_pid_line(&file)?,
        }),
        Err(err) => Err(err).with_context(|| format!("failed to lock {}", path.display())),
    }
}

fn held_message(published: &PidLine) -> String {
    match published {
        // The lock holder is alive by definition; a dead or missing PID line
        // is a diagnostic hint (startup in progress, or a foreign holder),
        // never a contradiction.
        PidLine::Valid(pid) if pid_is_alive(*pid) => {
            format!("daemon.pid is locked by a running daemon (pid {pid})")
        }
        PidLine::Valid(pid) => {
            format!("daemon.pid is locked by a running daemon (published pid {pid} is not running)")
        }
        PidLine::Invalid(raw) => {
            format!("daemon.pid is locked by a running daemon (published pid is unusable: '{raw}')")
        }
        PidLine::Missing => {
            "daemon.pid is locked by a running daemon (no published pid)".to_string()
        }
    }
}

fn read_pid_line(file: &File) -> Result<PidLine> {
    let mut raw = String::new();
    let mut handle = file;
    handle
        .read_to_string(&mut raw)
        .context("failed to read daemon lock file")?;

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(PidLine::Missing);
    }
    match trimmed.parse::<u32>() {
        Ok(pid) => Ok(PidLine::Valid(pid)),
        Err(_) => Ok(PidLine::Invalid(compact_value(trimmed))),
    }
}

/// Remove a stale lock file while still holding its `flock`, and only if the
/// path still names the inode this process locked: if the file was replaced
/// (a daemon started concurrently), leave the new file alone.
fn remove_stale_lock(path: &Path, stale: &StaleLock) -> Result<()> {
    let locked = stale
        .file
        .metadata()
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    let named = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if locked.dev() != named.dev() || locked.ino() != named.ino() {
        bail!("daemon.pid was replaced while it was being inspected; refusing to remove it");
    }
    fs::remove_file(path)
        .with_context(|| format!("failed removing stale daemon.pid: {}", path.display()))?;
    Ok(())
}

fn try_lock_exclusive(file: &File) -> std::io::Result<()> {
    // SAFETY: flock(2) operates only on the descriptor owned by `file`;
    // LOCK_NB keeps it non-blocking so doctor never waits on a live daemon.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    Err(std::io::Error::last_os_error())
}

fn compact_value(value: &str) -> String {
    let max_chars = 64;
    let len = value.chars().count();
    if len <= max_chars {
        return value.to_string();
    }

    let mut prefix: String = value.chars().take(max_chars).collect();
    prefix.push_str("...");
    prefix
}

fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    // SAFETY: kill(pid, 0) probes process existence/permission without sending a signal.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }

    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(code) if code == libc::EPERM
    )
}

#[cfg(test)]
#[path = "daemon_artifact_tests.rs"]
mod tests;
