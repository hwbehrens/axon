mod config;
mod daemon_artifacts;
mod peer_store;
mod state_root;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};

pub(super) use config::check_config;
pub(super) use daemon_artifacts::check_daemon_artifacts;
pub(super) use peer_store::check_peer_store;
pub(super) use state_root::check_state_root;

/// Monotonic tie-breaker for backups created within the same clock tick.
static BACKUP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn backup_file_with_timestamp(path: &Path) -> Result<PathBuf> {
    // Nanosecond stamp plus a process-monotonic sequence: a second backup in
    // the same second gets a distinct name instead of overwriting the first
    // via `rename`'s replace semantics.
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow!("system time error: {err}"))?
        .as_nanos();
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("path has no file name: {}", path.display()))?
        .to_string_lossy()
        .into_owned();
    let backup = loop {
        let seq = BACKUP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = path.with_file_name(format!("{file_name}.bak.{ts}.{seq}"));
        // Guards against a pre-existing file from an earlier process that
        // happened to reuse the same stamp and sequence.
        if !candidate.exists() {
            break candidate;
        }
    };
    fs::rename(path, &backup).with_context(|| {
        format!(
            "failed to back up {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(backup)
}
