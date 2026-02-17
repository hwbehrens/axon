use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};

pub(crate) const DAEMON_PID_FILE_NAME: &str = "daemon.pid";

pub(crate) struct DaemonLock {
    path: PathBuf,
    released: bool,
}

impl DaemonLock {
    pub(crate) fn acquire(state_root: &Path) -> Result<Self> {
        let path = state_root.join(DAEMON_PID_FILE_NAME);

        loop {
            match create_lock_file(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        released: false,
                    });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    let existing_pid = read_lock_pid(&path);
                    if let Some(pid) = existing_pid
                        && pid_is_alive(pid)
                    {
                        anyhow::bail!("daemon already running (pid {pid}) on this state root");
                    }
                    remove_stale_lock(&path, existing_pid)?;
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to create daemon lock file: {}", path.display())
                    });
                }
            }
        }
    }

    pub(crate) fn release(&mut self) -> Result<()> {
        self.release_inner()
            .with_context(|| format!("failed to remove daemon lock file: {}", self.path.display()))
    }

    fn release_inner(&mut self) -> std::io::Result<()> {
        if self.released {
            return Ok(());
        }
        remove_lock_file(&self.path)?;
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

fn create_lock_file(path: &Path) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    writeln!(file, "{}", std::process::id())?;
    file.sync_all()
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

fn remove_stale_lock(path: &Path, stale_pid: Option<u32>) -> Result<()> {
    match stale_pid {
        Some(pid) => info!(pid, path = %path.display(), "removing stale daemon lock file"),
        None => info!(path = %path.display(), "removing malformed daemon lock file"),
    }
    remove_lock_file(path).with_context(|| {
        format!(
            "failed to remove stale daemon lock file: {}",
            path.display()
        )
    })
}

fn remove_lock_file(path: &Path) -> std::io::Result<()> {
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

    // SAFETY: kill(pid, 0) does not send a signal; it only checks process existence/permissions.
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
#[path = "lockfile_tests.rs"]
mod tests;
