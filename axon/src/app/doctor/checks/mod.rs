mod config;
mod daemon_artifacts;
mod known_peers;
mod state_root;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};

pub(super) use config::check_config;
pub(super) use daemon_artifacts::check_daemon_artifacts;
pub(super) use known_peers::{check_duplicate_peer_addrs, check_known_peers};
pub(super) use state_root::check_state_root;

pub(super) fn backup_file_with_timestamp(path: &Path) -> Result<PathBuf> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow!("system time error: {err}"))?
        .as_secs();
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("path has no file name: {}", path.display()))?;
    let backup_name = format!("{}.bak.{ts}", file_name.to_string_lossy());
    let backup = path.with_file_name(backup_name);
    fs::rename(path, &backup).with_context(|| {
        format!(
            "failed to back up {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(backup)
}
