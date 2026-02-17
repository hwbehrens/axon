use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};

use axon::config::{AxonPaths, Config, load_known_peers, save_known_peers};

use super::{DoctorArgs, DoctorReport};

const DAEMON_PID_FILE_NAME: &str = "daemon.pid";

pub(super) fn check_state_root(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    if !paths.root.exists() {
        if args.fix {
            paths.ensure_root_exists()?;
            report.add_fix(
                "state_root_create",
                format!("created {}", paths.root.display()),
            );
            report.add_check(
                "state_root",
                true,
                true,
                format!("state root created at {}", paths.root.display()),
            );
        } else {
            report.add_check(
                "state_root",
                false,
                true,
                format!(
                    "state root does not exist: {} (run `axon doctor --fix` to create)",
                    paths.root.display()
                ),
            );
        }
        return Ok(());
    }

    let meta = fs::symlink_metadata(&paths.root)
        .with_context(|| format!("failed to read metadata: {}", paths.root.display()))?;
    if meta.file_type().is_symlink() {
        report.add_check(
            "state_root",
            false,
            false,
            format!(
                "state root is a symlink: {} (security violation; remove symlink manually)",
                paths.root.display()
            ),
        );
        return Ok(());
    }

    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        if args.fix {
            fs::set_permissions(&paths.root, fs::Permissions::from_mode(0o700)).with_context(
                || {
                    format!(
                        "failed to set state root permissions: {}",
                        paths.root.display()
                    )
                },
            )?;
            report.add_fix(
                "state_root_permissions",
                format!("set {} to 700", paths.root.display()),
            );
            report.add_check(
                "state_root",
                true,
                true,
                format!(
                    "state root permissions normalized to 700 ({})",
                    paths.root.display()
                ),
            );
        } else {
            report.add_check(
                "state_root",
                false,
                true,
                format!(
                    "state root permissions are {:o}, expected 700 ({})",
                    mode,
                    paths.root.display()
                ),
            );
        }
    } else {
        report.add_check(
            "state_root",
            true,
            false,
            format!("state root looks healthy ({})", paths.root.display()),
        );
    }

    Ok(())
}

pub(super) fn check_daemon_artifacts(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    let pid_path = paths.root.join(DAEMON_PID_FILE_NAME);
    let pid_state = inspect_daemon_pid(&pid_path)?;
    let pid_alive = matches!(&pid_state, DaemonPidState::Alive(_));

    match pid_state {
        DaemonPidState::Missing => report.add_check(
            "daemon_pid",
            true,
            false,
            "daemon.pid not present".to_string(),
        ),
        DaemonPidState::Alive(pid) => report.add_check(
            "daemon_pid",
            true,
            false,
            format!("daemon.pid points to running process {pid}"),
        ),
        DaemonPidState::Stale(pid) => {
            if args.fix {
                fs::remove_file(&pid_path).with_context(|| {
                    format!("failed removing stale daemon.pid: {}", pid_path.display())
                })?;
                report.add_fix(
                    "daemon_pid_cleanup",
                    format!("removed stale daemon.pid (pid {pid})"),
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
                    format!("stale daemon.pid references dead pid {pid}"),
                );
            }
        }
        DaemonPidState::Invalid(raw) => {
            if args.fix {
                fs::remove_file(&pid_path).with_context(|| {
                    format!("failed removing invalid daemon.pid: {}", pid_path.display())
                })?;
                report.add_fix(
                    "daemon_pid_cleanup",
                    format!("removed invalid daemon.pid contents '{raw}'"),
                );
                report.add_check(
                    "daemon_pid",
                    true,
                    true,
                    "removed invalid daemon.pid".to_string(),
                );
            } else {
                report.add_check(
                    "daemon_pid",
                    false,
                    true,
                    format!("daemon.pid contains invalid pid value '{raw}'"),
                );
            }
        }
    }

    if !paths.socket.exists() {
        if pid_alive {
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

    if pid_alive {
        report.add_check(
            "ipc_socket",
            true,
            false,
            format!("socket present ({})", paths.socket.display()),
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
                "socket exists but no live daemon pid found: {}",
                paths.socket.display()
            ),
        );
    }

    Ok(())
}

pub(super) async fn check_known_peers(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    if !paths.known_peers.exists() {
        report.add_check(
            "known_peers",
            true,
            false,
            "known_peers.json not present".to_string(),
        );
        return Ok(());
    }

    match load_known_peers(&paths.known_peers).await {
        Ok(peers) => {
            report.add_check(
                "known_peers",
                true,
                false,
                format!("known_peers.json parsed ({} entries)", peers.len()),
            );
        }
        Err(err) => {
            if args.fix {
                let backup = backup_file_with_timestamp(&paths.known_peers)?;
                save_known_peers(&paths.known_peers, &[]).await?;
                report.add_fix(
                    "known_peers_reset",
                    format!(
                        "backed up corrupt known_peers.json to {} and reset to []",
                        backup.display()
                    ),
                );
                report.add_check(
                    "known_peers",
                    true,
                    true,
                    "corrupt known_peers.json reset".to_string(),
                );
            } else {
                report.add_check(
                    "known_peers",
                    false,
                    true,
                    format!(
                        "known_peers.json is not parseable ({err}); run `axon doctor --fix` to back up and reset"
                    ),
                );
            }
        }
    }

    Ok(())
}

pub(super) async fn check_config(paths: &AxonPaths, report: &mut DoctorReport) -> Result<()> {
    if !paths.config.exists() {
        report.add_check("config", true, false, "config.toml not present".to_string());
        return Ok(());
    }

    match Config::load(&paths.config).await {
        Ok(cfg) => {
            report.add_check(
                "config",
                true,
                false,
                format!("config.toml parsed ({} static peers)", cfg.peers.len()),
            );
        }
        Err(err) => {
            report.add_check(
                "config",
                false,
                false,
                format!("config.toml parse/load error: {err}"),
            );
        }
    }

    Ok(())
}

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

#[derive(Debug, Clone)]
enum DaemonPidState {
    Missing,
    Alive(u32),
    Stale(u32),
    Invalid(String),
}

fn inspect_daemon_pid(path: &Path) -> Result<DaemonPidState> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(DaemonPidState::Missing),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(DaemonPidState::Invalid("empty".to_string()));
    }

    let pid = match trimmed.parse::<u32>() {
        Ok(pid) => pid,
        Err(_) => return Ok(DaemonPidState::Invalid(compact_value(trimmed))),
    };

    if pid_is_alive(pid) {
        Ok(DaemonPidState::Alive(pid))
    } else {
        Ok(DaemonPidState::Stale(pid))
    }
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
