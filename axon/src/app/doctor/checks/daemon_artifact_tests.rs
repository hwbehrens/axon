//! Daemon-artifact tests: the single-instance lock file and the IPC socket.
//!
//! The lock's authority is the daemon's `flock(2)`, not its PID line, so
//! these tests hold a real flock to represent a running daemon, pin that a
//! live-but-unrelated PID without the flock is manual-only (never deleted by
//! fix mode, because the daemon's own startup backstop refuses to start
//! beside it), and pin that dead or malformed records stay auto-fixable.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use tempfile::tempdir;

use axon::config::AxonPaths;

use crate::app::doctor::{DoctorArgs, DoctorReport};

use super::*;

fn args(fix: bool) -> DoctorArgs {
    DoctorArgs {
        json: false,
        fix,
        rekey: false,
    }
}

fn check_named<'a>(report: &'a DoctorReport, name: &str) -> &'a crate::app::doctor::DoctorCheck {
    report
        .checks
        .iter()
        .find(|check| check.name == name)
        .unwrap_or_else(|| panic!("report must contain a {name} check"))
}

/// A well-formed pid that is virtually never running on any platform.
fn dead_pid() -> u32 {
    1_999_999_999
}

/// Hold the daemon lock the way a running daemon does. The returned handle
/// must stay alive: dropping it releases the flock.
fn hold_daemon_lock(path: &Path) -> File {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .expect("out of scope: probe lock file");
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "test must be able to hold the daemon lock");
    file
}

#[tokio::test]
async fn absent_pid_and_socket_report_ok() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    assert!(report.ok);
    assert!(check_named(&report, "daemon_pid").ok);
    assert!(check_named(&report, "ipc_socket").ok);
}

#[tokio::test]
async fn held_lock_without_socket_flags_missing_ipc_endpoint() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let _lock = hold_daemon_lock(&paths.daemon_lock);
    fs::write(&paths.daemon_lock, std::process::id().to_string()).expect("pid line");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    assert!(check_named(&report, "daemon_pid").ok);
    let socket_check = check_named(&report, "ipc_socket");
    assert!(!socket_check.ok, "running daemon must have a socket");
    assert!(!socket_check.fixable);
    assert!(!report.ok);
}

#[tokio::test]
async fn held_lock_with_socket_is_healthy() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let _lock = hold_daemon_lock(&paths.daemon_lock);
    fs::write(&paths.daemon_lock, std::process::id().to_string()).expect("pid line");
    let _listener = tokio::net::UnixListener::bind(&paths.socket).expect("bind socket");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    assert!(report.ok);
    assert!(check_named(&report, "ipc_socket").ok);
}

/// R8/R-DOCTOR: a PID line naming a live process is not evidence of a
/// running daemon, but it is also the exact state the daemon's own startup
/// backstop refuses to start beside. Fix mode must therefore leave the
/// record and the socket alone; only once the process is gone does the same
/// record become a stale artifact fix mode may remove.
#[tokio::test]
async fn live_pid_without_the_lock_is_manual_only() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    // This process is a live PID that does not hold the lock. Do not fork a
    // helper: a child inherits any flock descriptor another test thread holds
    // and keeps that lock alive until it execs, which makes parallel flock
    // tests flaky.
    let live_pid = std::process::id();
    fs::write(&paths.daemon_lock, live_pid.to_string()).expect("pid file");
    // A socket that may belong to the still-running pre-flock daemon.
    let listener = tokio::net::UnixListener::bind(&paths.socket).expect("bind socket");
    drop(listener);
    assert!(paths.socket.exists());

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    let pid_check = check_named(&report, "daemon_pid");
    assert!(
        !pid_check.ok,
        "a live PID without the flock must not report a running daemon"
    );
    assert!(
        !pid_check.fixable,
        "doctor must not delete a live-PID guard record"
    );
    assert!(
        pid_check
            .message
            .contains(&format!("live process {live_pid}")),
        "the reason must name the live process: {}",
        pid_check.message
    );
    assert!(
        !check_named(&report, "ipc_socket").fixable,
        "the socket may belong to the recorded daemon"
    );
    assert!(!report.ok);

    // Fix mode must leave both artifacts untouched: deleting them would let
    // a second daemon start beside the recorded live process, which the
    // daemon's own backstop refuses to do.
    let mut fix_report = DoctorReport::new(&paths, true);
    check_daemon_artifacts(&paths, &args(true), &mut fix_report).expect("fix runs");
    assert!(
        fix_report.fixes_applied.is_empty(),
        "fix mode must not touch a live-PID lock or its socket: {:?}",
        fix_report.fixes_applied
    );
    assert!(!fix_report.ok, "manual-only findings keep failing closed");
    assert!(paths.daemon_lock.exists(), "fix must not remove the guard");
    assert!(
        paths.socket.exists(),
        "fix must not remove the live daemon's socket"
    );

    // Once the process is gone the same record is provably stale.
    fs::write(&paths.daemon_lock, dead_pid().to_string()).expect("dead pid file");

    let mut second_fix = DoctorReport::new(&paths, true);
    check_daemon_artifacts(&paths, &args(true), &mut second_fix).expect("fix runs");
    assert!(second_fix.ok);
    assert!(!paths.daemon_lock.exists());
    assert!(!paths.socket.exists());
}

/// The flock wins over a stale or recycled PID line: the daemon is running
/// even though the published PID is dead or unparseable.
#[tokio::test]
async fn held_lock_wins_over_a_stale_pid_line() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let _lock = hold_daemon_lock(&paths.daemon_lock);
    fs::write(&paths.daemon_lock, dead_pid().to_string()).expect("pid line");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    let pid_check = check_named(&report, "daemon_pid");
    assert!(pid_check.ok, "a held flock proves a live daemon");
    assert!(
        pid_check.message.contains("locked by a running daemon"),
        "the message must cite the lock: {}",
        pid_check.message
    );
    assert!(
        pid_check.message.contains("is not running"),
        "the dead published PID is reported as a diagnostic: {}",
        pid_check.message
    );
}

#[tokio::test]
async fn stale_pid_and_orphaned_socket_are_fixable() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(&paths.daemon_lock, dead_pid().to_string()).expect("pid file");
    // Bind then drop: the socket file outlives its listener, like a crashed daemon.
    let listener = tokio::net::UnixListener::bind(&paths.socket).expect("bind socket");
    drop(listener);
    assert!(paths.socket.exists());

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");
    assert!(!report.ok);
    assert!(check_named(&report, "daemon_pid").fixable);
    assert!(check_named(&report, "ipc_socket").fixable);

    let mut fix_report = DoctorReport::new(&paths, true);
    check_daemon_artifacts(&paths, &args(true), &mut fix_report).expect("fix runs");
    assert!(fix_report.ok);
    assert_eq!(
        fix_report.fixes_applied.len(),
        2,
        "pid and socket both cleaned"
    );
    assert!(!paths.daemon_lock.exists());
    assert!(!paths.socket.exists());

    // Convergence: after fixing, a fresh check is healthy.
    let mut recheck = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut recheck).expect("recheck runs");
    assert!(recheck.ok);
}

#[tokio::test]
async fn invalid_pid_contents_are_reported_and_removed_by_fix() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(&paths.daemon_lock, "not-a-pid at all").expect("pid file");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");
    let pid_check = check_named(&report, "daemon_pid");
    assert!(!pid_check.ok);
    assert!(pid_check.message.contains("not-a-pid at all"));

    let mut fix_report = DoctorReport::new(&paths, true);
    check_daemon_artifacts(&paths, &args(true), &mut fix_report).expect("fix runs");
    assert!(check_named(&fix_report, "daemon_pid").ok);
    assert!(!paths.daemon_lock.exists());
}

#[tokio::test]
async fn non_regular_lock_path_requires_manual_cleanup() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::create_dir(&paths.daemon_lock).expect("directory at lock path");

    let mut report = DoctorReport::new(&paths, true);
    check_daemon_artifacts(&paths, &args(true), &mut report).expect("check runs");

    let pid_check = check_named(&report, "daemon_pid");
    assert!(!pid_check.ok);
    assert!(!pid_check.fixable, "deleting arbitrary paths is manual");
    assert!(
        paths.daemon_lock.is_dir(),
        "fix mode must not delete non-regular paths"
    );
}

#[tokio::test]
async fn regular_file_at_socket_path_requires_manual_cleanup() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let socket_path = paths.socket.clone();
    fs::write(&socket_path, "not a socket").expect("regular file");

    let mut report = DoctorReport::new(&paths, true);
    check_daemon_artifacts(&paths, &args(true), &mut report).expect("check runs");

    let socket_check = check_named(&report, "ipc_socket");
    assert!(!socket_check.ok);
    assert!(!socket_check.fixable, "deleting arbitrary files is manual");
    assert!(
        socket_path.exists(),
        "fix mode must not delete regular files"
    );
}

#[tokio::test]
async fn long_invalid_pid_values_are_compacted_in_findings() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let long_garbage = "x".repeat(200);
    fs::write(&paths.daemon_lock, &long_garbage).expect("pid file");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    let message = check_named(&report, "daemon_pid").message.clone();
    assert!(
        message.len() < long_garbage.len(),
        "oversized values must be compacted before landing in the report"
    );
    assert!(message.contains("..."));
}

#[tokio::test]
async fn unreadable_pid_file_fails_the_check_with_context() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(&paths.daemon_lock, "1234").expect("pid file");
    fs::set_permissions(&paths.daemon_lock, fs::Permissions::from_mode(0o000)).expect("chmod");
    if fs::read(&paths.daemon_lock).is_err() {
        // Only assert where permission bits are enforced (not running as root).
        let result = {
            let mut report = DoctorReport::new(&paths, false);
            check_daemon_artifacts(&paths, &args(false), &mut report)
        };
        assert!(
            result.is_err(),
            "an unreadable pid file must surface as a check failure with context"
        );
    }
    let _ = fs::set_permissions(&paths.daemon_lock, fs::Permissions::from_mode(0o644));
}
