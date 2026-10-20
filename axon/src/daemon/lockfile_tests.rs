use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

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
fn lock_file_naming_this_process_is_replaced() {
    // A PID line naming this very process is our own crashed record, not a
    // foreign live daemon, so the transitional backstop must not trip and
    // the flock holder replaces it.
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    fs::write(&path, format!("{}\n", std::process::id())).unwrap();

    let mut lock = DaemonLock::acquire(&path).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert_eq!(raw.trim(), std::process::id().to_string());

    lock.release().unwrap();
}

#[test]
fn lock_file_naming_a_live_foreign_process_fails_closed() {
    // Transitional backstop: a pre-flock daemon published its PID here but
    // held no flock, so the flock alone must not admit a second daemon whose
    // PID is demonstrably alive and different.
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn a live sleeper");
    fs::write(&path, format!("{}\n", child.id())).unwrap();

    let err = match DaemonLock::acquire(&path) {
        Ok(_) => panic!("a live foreign PID must fail closed"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("refusing to start"),
        "unexpected error: {err:#}"
    );
    assert!(path.exists(), "a refused acquire must keep the lock path");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{}\n", child.id()),
        "a refused acquire must not overwrite the recorded PID"
    );

    // Once the recorded process is gone, the same record is provably stale.
    child.kill().unwrap();
    child.wait().unwrap();
    let mut lock = DaemonLock::acquire(&path).expect("a dead PID is stale");
    lock.release().unwrap();
}

#[test]
fn release_restores_a_successor_file_swapped_mid_release() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    let mut lock = DaemonLock::acquire(&path).unwrap();

    // Force the TOCTOU window the capture design closes: a successor's file
    // replaces the path after this process decided the lock was still its
    // own, just where the old check-then-unlink would have removed it.
    let swapped = path.clone();
    super::release_hook::install(move || {
        fs::remove_file(&swapped).unwrap();
        fs::write(&swapped, "successor\n").unwrap();
    });

    lock.release().unwrap();
    assert!(
        path.exists(),
        "release must not remove a successor's lock file"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "successor\n");
}

#[test]
fn release_is_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    let mut lock = DaemonLock::acquire(&path).unwrap();
    lock.release().unwrap();
    lock.release().expect("a second release is a no-op");
    assert!(!path.exists());
}

#[test]
fn malformed_lock_file_is_replaced_without_unverified_removal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    fs::write(&path, "not-a-pid\n").unwrap();

    let mut lock = DaemonLock::acquire(&path).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert_eq!(raw.trim(), std::process::id().to_string());

    lock.release().unwrap();
}

#[test]
fn second_acquire_is_rejected_while_first_is_held() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);

    let mut first = DaemonLock::acquire(&path).unwrap();
    let err = match DaemonLock::acquire(&path) {
        Ok(_) => panic!("a held lock must reject a second acquirer"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("daemon already running"),
        "unexpected error: {err:#}"
    );
    assert!(path.exists(), "a held lock must not be removed");
    first.release().unwrap();
    assert!(!path.exists());
}

#[test]
fn release_does_not_unlink_a_replaced_lock_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);

    let mut lock = DaemonLock::acquire(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, "replacement\n").unwrap();

    lock.release().unwrap();
    assert!(
        path.exists(),
        "release must not unlink a lock file it no longer owns"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "replacement\n");
}

#[test]
fn acquire_refuses_non_regular_lock_path() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);
    fs::create_dir(&path).unwrap();

    let err = match DaemonLock::acquire(&path) {
        Ok(_) => panic!("directory lock path must be refused"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("non-regular"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn concurrent_acquire_admits_exactly_one_holder() {
    const THREADS: usize = 8;
    const ROUNDS: usize = 25;
    let dir = tempdir().unwrap();
    let path = dir.path().join(LOCK_FILE_NAME);

    for round in 0..ROUNDS {
        // Re-seed the historical race: an empty file that concurrent
        // acquirers used to remove and recreate, letting two of them hold
        // what each believed was the single-instance lock.
        fs::write(&path, "").unwrap();

        let barrier = Arc::new(Barrier::new(THREADS));
        let simultaneous = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let path = path.clone();
            let barrier = barrier.clone();
            let simultaneous = simultaneous.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let mut lock = DaemonLock::acquire(&path).ok();
                if lock.is_some() {
                    simultaneous.fetch_add(1, Ordering::SeqCst);
                }
                // Hold the lock until every other attempt has resolved, so
                // the count is the number of simultaneous holders.
                barrier.wait();
                if let Some(lock) = lock.as_mut() {
                    lock.release().unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            simultaneous.load(Ordering::SeqCst),
            1,
            "round {round}: exactly one daemon may hold the lock"
        );
    }
}
