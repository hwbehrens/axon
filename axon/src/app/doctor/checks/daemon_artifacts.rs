use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

use anyhow::{Context, Result};

use axon::config::AxonPaths;

use crate::app::doctor::{DoctorArgs, DoctorReport};

const DAEMON_PID_FILE_NAME: &str = "daemon.pid";

pub(in crate::app::doctor) fn check_daemon_artifacts(
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
