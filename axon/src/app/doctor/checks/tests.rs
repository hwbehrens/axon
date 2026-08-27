use std::fs;
use std::os::unix::fs::PermissionsExt;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tempfile::tempdir;

use axon::config::{AxonPaths, PersistedConfig, load_persisted_config};
use axon::message::AgentId;

use super::*;
use crate::app::doctor::{DoctorArgs, DoctorReport};

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

// ---------------------------------------------------------------------------
// state_root
// ---------------------------------------------------------------------------

#[test]
fn missing_state_root_is_actionable_and_fix_creates_it() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().join("axon-state"));

    let mut report = DoctorReport::new(&paths, false);
    check_state_root(&paths, &args(false), &mut report).expect("check runs");
    assert!(!report.ok);
    assert!(check_named(&report, "state_root").fixable);

    let mut fix_report = DoctorReport::new(&paths, true);
    check_state_root(&paths, &args(true), &mut fix_report).expect("fix runs");
    assert!(fix_report.ok);
    assert!(paths.root.is_dir());
    assert_eq!(
        fs::symlink_metadata(&paths.root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn symlinked_state_root_is_rejected_without_fix() {
    let root = tempdir().expect("tempdir");
    let real = root.path().join("real");
    fs::create_dir_all(&real).expect("real dir");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let paths = AxonPaths::from_root(link);

    let mut report = DoctorReport::new(&paths, true);
    check_state_root(&paths, &args(true), &mut report).expect("check runs");

    let check = check_named(&report, "state_root");
    assert!(!check.ok);
    assert!(!check.fixable, "symlink removal is manual by design");
    assert!(report.fixes_applied.is_empty());
}

#[test]
fn loose_permissions_are_reported_and_normalized_by_fix() {
    let root = tempdir().expect("tempdir");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).expect("chmod");
    let paths = AxonPaths::from_root(root.path().to_path_buf());

    let mut report = DoctorReport::new(&paths, false);
    check_state_root(&paths, &args(false), &mut report).expect("check runs");
    assert!(!report.ok);

    let mut fix_report = DoctorReport::new(&paths, true);
    check_state_root(&paths, &args(true), &mut fix_report).expect("fix runs");
    assert!(fix_report.ok);
    assert_eq!(
        fs::symlink_metadata(root.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn healthy_state_root_reports_ok() {
    let root = tempdir().expect("tempdir");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("chmod");
    let paths = AxonPaths::from_root(root.path().to_path_buf());

    let mut report = DoctorReport::new(&paths, false);
    check_state_root(&paths, &args(false), &mut report).expect("check runs");

    assert!(report.ok);
    assert!(check_named(&report, "state_root").ok);
}

// ---------------------------------------------------------------------------
// daemon artifacts (pid file + ipc socket)
// ---------------------------------------------------------------------------

/// A live pid for probe purposes: this test process itself.
fn live_pid() -> u32 {
    std::process::id()
}

/// A well-formed pid that is virtually never running on any platform.
fn dead_pid() -> u32 {
    1_999_999_999
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
async fn alive_pid_without_socket_flags_missing_ipc_endpoint() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(paths.root.join("daemon.pid"), live_pid().to_string()).expect("pid file");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    let socket_check = check_named(&report, "ipc_socket");
    assert!(!socket_check.ok, "running daemon must have a socket");
    assert!(!socket_check.fixable);
    assert!(!report.ok);
}

#[tokio::test]
async fn alive_pid_with_socket_is_healthy() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(paths.root.join("daemon.pid"), live_pid().to_string()).expect("pid file");
    let _listener = tokio::net::UnixListener::bind(&paths.socket).expect("bind socket");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");

    assert!(report.ok);
    assert!(check_named(&report, "ipc_socket").ok);
}

#[tokio::test]
async fn stale_pid_and_orphaned_socket_are_fixable() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(paths.root.join("daemon.pid"), dead_pid().to_string()).expect("pid file");
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
    assert!(!paths.root.join("daemon.pid").exists());
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
    fs::write(paths.root.join("daemon.pid"), "not-a-pid at all").expect("pid file");

    let mut report = DoctorReport::new(&paths, false);
    check_daemon_artifacts(&paths, &args(false), &mut report).expect("check runs");
    let pid_check = check_named(&report, "daemon_pid");
    assert!(!pid_check.ok);
    assert!(pid_check.message.contains("not-a-pid at all"));

    let mut fix_report = DoctorReport::new(&paths, true);
    check_daemon_artifacts(&paths, &args(true), &mut fix_report).expect("fix runs");
    assert!(check_named(&fix_report, "daemon_pid").ok);
    assert!(!paths.root.join("daemon.pid").exists());
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

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_config_is_healthy() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());

    let mut report = DoctorReport::new(&paths, false);
    check_config(&paths, &args(false), &mut report)
        .await
        .expect("check runs");

    assert!(report.ok);
    assert!(check_named(&report, "config").ok);
}

#[tokio::test]
async fn valid_config_parses_cleanly() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(&paths.config, "name: alice\nport: 7200\n").expect("config");

    let mut report = DoctorReport::new(&paths, false);
    check_config(&paths, &args(false), &mut report)
        .await
        .expect("check runs");

    assert!(report.ok);
    let check = check_named(&report, "config");
    assert!(check.message.contains("alice"));
    assert!(check.message.contains("7200"));
}

#[tokio::test]
async fn corrupt_config_is_backed_up_and_reset_by_fix() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(&paths.config, "peers: [legacy]\n\tbroken: [").expect("corrupt config");

    let mut report = DoctorReport::new(&paths, false);
    check_config(&paths, &args(false), &mut report)
        .await
        .expect("check runs");
    assert!(!report.ok);
    assert!(check_named(&report, "config").fixable);

    let mut fix_report = DoctorReport::new(&paths, true);
    check_config(&paths, &args(true), &mut fix_report)
        .await
        .expect("fix runs");
    assert!(fix_report.ok);
    assert_eq!(fix_report.fixes_applied.len(), 1);
    // The corrupt original survives as a backup; live config is defaults.
    let backups: Vec<_> = fs::read_dir(paths.config.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.contains(".bak."))
        .collect();
    assert_eq!(backups.len(), 1, "corrupt config must be backed up");
    let reset = load_persisted_config(&paths.config)
        .await
        .expect("reset config loads");
    assert_eq!(reset, PersistedConfig::default());
}

// ---------------------------------------------------------------------------
// peer store / legacy state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legacy_known_peers_is_flagged_and_archived_by_fix() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(&paths.legacy_known_peers, "{}").expect("legacy cache");

    let mut report = DoctorReport::new(&paths, false);
    check_peer_store(&paths, &args(false), &mut report)
        .await
        .expect("check runs");
    assert!(!report.ok);
    assert!(check_named(&report, "legacy_peer_state").fixable);
    assert!(
        paths.legacy_known_peers.exists(),
        "check mode never mutates"
    );

    let mut fix_report = DoctorReport::new(&paths, true);
    check_peer_store(&paths, &args(true), &mut fix_report)
        .await
        .expect("fix runs");
    assert!(check_named(&fix_report, "legacy_peer_state").ok);
    assert!(
        !paths.legacy_known_peers.exists(),
        "legacy cache moves aside"
    );
}

#[tokio::test]
async fn invalid_peer_store_fails_closed_with_guidance() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    fs::write(&paths.peers, "{\"version\":1,\"peers\":[{}]}").expect("invalid store");

    let mut report = DoctorReport::new(&paths, false);
    check_peer_store(&paths, &args(false), &mut report)
        .await
        .expect("check runs");

    let check = check_named(&report, "peer_store");
    assert!(!check.ok);
    assert!(!check.fixable, "trust data is never auto-deleted");
    assert!(check.message.contains("fail closed"));
}

#[tokio::test]
async fn enrolled_count_is_surfaced_for_valid_store() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let key = STANDARD.encode([3u8; 32]);
    let agent_id = AgentId::from_pubkey_base64(&key).expect("valid test key");
    let document = serde_json::json!({
        "version": 1,
        "peers": [{
            "agent_id": agent_id.as_str(),
            "pubkey": key,
            "locators": [],
        }],
    });
    std::fs::write(
        &paths.peers,
        serde_json::to_vec_pretty(&document).expect("encode store"),
    )
    .expect("seed store");

    let mut report = DoctorReport::new(&paths, false);
    check_peer_store(&paths, &args(false), &mut report)
        .await
        .expect("check runs");

    let check = check_named(&report, "peer_store");
    assert!(check.ok);
    assert!(check.message.contains("1 enrolled peers"));
}

#[tokio::test]
async fn long_invalid_pid_values_are_compacted_in_findings() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let long_garbage = "x".repeat(200);
    fs::write(paths.root.join("daemon.pid"), &long_garbage).expect("pid file");

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
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    let pid_path = paths.root.join("daemon.pid");
    fs::write(&pid_path, "1234").expect("pid file");
    fs::set_permissions(&pid_path, fs::Permissions::from_mode(0o000)).expect("chmod");
    if fs::read(&pid_path).is_err() {
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
    let _ = fs::set_permissions(&pid_path, fs::Permissions::from_mode(0o644));
}

#[tokio::test]
async fn symlinked_root_aborts_doctor_before_other_checks_touch_it() {
    let root = tempdir().expect("tempdir");
    let real = root.path().join("victim");
    fs::create_dir_all(&real).expect("victim dir");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let paths = AxonPaths::from_root(link);

    let report = crate::app::doctor::run(&paths, &args(true))
        .await
        .expect("doctor runs");

    // Only state_root may report; --fix must not write key material or any
    // other artifact through the attacker-controlled directory.
    assert!(!report.ok);
    let non_root_findings: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.name != "state_root")
        .collect();
    assert!(
        non_root_findings.is_empty(),
        "doctor must stop at a rejected symlinked root, found {non_root_findings:?}"
    );
    assert!(report.fixes_applied.is_empty());
    assert!(
        fs::read_dir(&real).expect("victim dir").count() == 0,
        "nothing may be written through the symlink"
    );
}
