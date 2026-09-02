use std::fs;

use tempfile::tempdir;

use super::DaemonLock;

const LOCK_FILE_NAME: &str = "daemon.pid";

#[test]
fn acquire_creates_lock_with_current_pid() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);

    let mut lock = DaemonLock::acquire(&path).unwrap();

    assert!(path.exists(), "daemon lock should exist after acquire");
    let raw = fs::read_to_string(&path).unwrap();
    assert_eq!(raw.trim(), std::process::id().to_string());

    lock.release().unwrap();
    assert!(!path.exists(), "daemon lock should be removed on release");
}

#[test]
fn stale_lock_is_replaced() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    fs::write(&path, format!("{}\n", u32::MAX)).unwrap();

    let mut lock = DaemonLock::acquire(&path).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert_eq!(raw.trim(), std::process::id().to_string());

    lock.release().unwrap();
}

#[test]
fn lock_with_live_pid_is_rejected() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    fs::write(&path, format!("{}\n", std::process::id())).unwrap();

    let err = match DaemonLock::acquire(&path) {
        Ok(_) => panic!("live lock should be rejected"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("daemon already running"),
        "unexpected error: {err:#}"
    );
    assert!(path.exists(), "live lock should not be removed");
}
