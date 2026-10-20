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
// backups
// ---------------------------------------------------------------------------

#[test]
fn backups_in_the_same_second_do_not_collide() {
    let root = tempdir().expect("tempdir");
    let config = root.path().join("config.yaml");
    fs::write(&config, "first").expect("first config");
    let first = backup_file_with_timestamp(&config).expect("first backup");
    fs::write(&config, "second").expect("second config");
    let second = backup_file_with_timestamp(&config).expect("second backup");

    assert_ne!(first, second, "backup names must not collide");
    assert_eq!(fs::read_to_string(&first).unwrap(), "first");
    assert_eq!(fs::read_to_string(&second).unwrap(), "second");
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
async fn semantically_invalid_config_fails_the_check() {
    for invalid in ["port: 0\n", "name: \"\"\n", "name: \"  \"\n"] {
        let root = tempdir().expect("tempdir");
        let paths = AxonPaths::from_root(root.path().to_path_buf());
        fs::write(&paths.config, invalid).expect("config");

        let mut report = DoctorReport::new(&paths, false);
        check_config(&paths, &args(false), &mut report)
            .await
            .expect("check runs");

        let check = check_named(&report, "config");
        assert!(
            !check.ok,
            "config {invalid:?} must fail the doctor check, got: {check:?}"
        );
        assert!(check.fixable, "reset-to-defaults is the offered repair");
    }
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
