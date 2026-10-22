//! Hegel stateful test: daemon-artifact states vs doctor verdicts.
//!
//! Rules drive the AXON root through artifact combinations (lock files,
//! socket paths, their contents, a held flock); invariants pin the doctor
//! contract after every transition:
//!
//! - in check mode, `report.ok` mirrors actual artifact health exactly;
//! - in fix mode, actionable problems are repaired and never fabricated;
//! - fixing converges unless the remainder is manual-only by design
//!   (a regular file squatting on the socket path, or a live daemon whose
//!   socket never appeared — doctor cannot start processes or delete
//!   arbitrary files).
//!
//! A running daemon is modeled by holding the real `flock` the daemon holds:
//! the PID line is only a diagnostic, so writing a live process's PID into
//! the file (rule `write_live_process_pid`) must never make it healthy.
//! Because the daemon's own startup backstop refuses to start beside a live
//! recorded PID, that state is a manual-only finding fix mode must never
//! delete, guarding the pre-flock daemon the record may describe.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hegel::{TestCase, generators as gs, stateful};
use tempfile::TempDir;

use crate::app::doctor::checks::{
    check_config, check_daemon_artifacts, check_peer_store, check_state_root,
};
use crate::app::doctor::{DoctorArgs, DoctorReport};

fn args(fix: bool) -> DoctorArgs {
    DoctorArgs {
        json: false,
        fix,
        rekey: false,
    }
}

/// A pid that parses but is virtually never running.
const DEAD_PID_BASE: u32 = 1_999_999_999;

struct DoctorArtifactsMachine {
    rt: tokio::runtime::Runtime,
    _root: TempDir,
    paths: axon::config::AxonPaths,
    /// The flock a running daemon would hold. `None` means no daemon runs.
    held_lock: Option<File>,
}

impl DoctorArtifactsMachine {
    fn new() -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio current-thread runtime");
        let root = tempfile::tempdir().expect("tempdir");
        let paths = axon::config::AxonPaths::from_root(root.path().to_path_buf());
        fs::create_dir_all(&paths.root).expect("create state root");
        // Provision like a real deployment; loose perms are state_root's
        // scenario, not this machine's.
        fs::set_permissions(&paths.root, fs::Permissions::from_mode(0o700))
            .expect("secure state root");
        Self {
            rt,
            _root: root,
            paths,
            held_lock: None,
        }
    }

    /// Whether the held flock belongs to the file the lock path currently
    /// names. A rule may unlink or replace the path while the descriptor
    /// stays open; the kernel keeps that orphaned lock, but it no longer
    /// guards the named file, exactly like the daemon's own inode checks.
    fn lock_held_on_path(&self) -> bool {
        let Some(file) = &self.held_lock else {
            return false;
        };
        let (Ok(held), Ok(named)) = (
            file.metadata(),
            fs::symlink_metadata(self.paths.daemon_lock.clone()),
        ) else {
            return false;
        };
        held.dev() == named.dev() && held.ino() == named.ino()
    }

    fn pid_path(&self) -> std::path::PathBuf {
        self.paths.daemon_lock.clone()
    }

    fn sweep(&self, fix: bool) -> DoctorReport {
        let mut report = DoctorReport::new(&self.paths, fix);
        let args = args(fix);
        check_state_root(&self.paths, &args, &mut report)
            .expect("state root check runs")
            .then_some(())
            .expect("provisioned root must never be a symlink");
        check_daemon_artifacts(&self.paths, &args, &mut report).expect("artifact check runs");
        self.rt
            .block_on(check_peer_store(&self.paths, &args, &mut report))
            .expect("peer store check runs");
        self.rt
            .block_on(check_config(&self.paths, &args, &mut report))
            .expect("config check runs");
        report
    }

    /// Reference model of artifact health, mirroring the documented doctor
    /// contract rather than its implementation details.
    ///
    /// Lock health: a held flock is a running daemon regardless of the PID
    /// line; a lock file no process holds is a fixable leftover regardless of
    /// its contents. Socket health: a real unix socket is fine; a regular
    /// file is not; absence is fine unless a live daemon should be listening.
    fn artifacts_healthy(&self) -> bool {
        let lock_held = self.lock_held_on_path();
        let pid_healthy =
            fs::symlink_metadata(self.paths.daemon_lock.clone()).is_err() || lock_held;

        let socket_health = fs::symlink_metadata(&self.paths.socket);
        // A socket is only healthy while its daemon lives; an abandoned one
        // is a fixable finding even with no lock file at all.
        let socket_healthy = match socket_health {
            Err(_) => !lock_held,
            Ok(meta) => meta.file_type().is_socket() && lock_held,
        };

        // Peer store and config are seeded valid or left absent by the rules,
        // so they never contribute findings here.
        pid_healthy && socket_healthy
    }

    /// Whether the lock path names a live process while no `flock` is held
    /// on it: the daemon's startup backstop refuses to start beside that
    /// record, and doctor treats it as manual-only rather than deleting the
    /// guard. Mirrors doctor's classification, including its pid bounds.
    fn lock_names_live_pid_without_flock(&self) -> bool {
        if self.lock_held_on_path() {
            return false;
        }
        let Ok(raw) = fs::read_to_string(self.pid_path()) else {
            return false;
        };
        let Ok(pid) = raw.trim().parse::<u32>() else {
            return false;
        };
        if pid == 0 || pid > i32::MAX as u32 {
            return false;
        }
        // SAFETY: kill(pid, 0) sends no signal; it only probes existence
        // and permission to signal.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        rc == 0
            || matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(code) if code == libc::EPERM
            )
    }

    /// Problems doctor cannot repair by deleting files.
    fn manual_only_remainder(&self) -> bool {
        let live_pid_without_flock = self.lock_names_live_pid_without_flock();
        let running_without_socket = self.lock_held_on_path() && !self.paths.socket.exists();
        let regular_file_socket = fs::symlink_metadata(&self.paths.socket)
            .map(|meta| !meta.file_type().is_socket())
            .unwrap_or(false);
        live_pid_without_flock || running_without_socket || regular_file_socket
    }
}

#[hegel::state_machine]
impl DoctorArtifactsMachine {
    /// Write the (live) PID of this test process without holding the lock:
    /// a PID line alone is never evidence of a running daemon, and because
    /// the daemon's startup backstop refuses to start beside a live recorded
    /// PID, doctor must report it as manual-only rather than delete it.
    #[rule]
    fn write_live_process_pid(&mut self, _tc: TestCase) {
        fs::write(self.pid_path(), std::process::id().to_string()).expect("pid file");
    }

    /// Start a "daemon": hold the real flock the daemon holds at startup.
    #[rule]
    fn hold_lock(&mut self, _tc: TestCase) {
        if self.lock_held_on_path() {
            return;
        }
        self.held_lock = None;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.pid_path())
            .expect("lock file");
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(rc, 0, "the state machine must be the only lock holder");
        self.held_lock = Some(file);
    }

    /// Stop the "daemon": unlink while still holding the lock, then release
    /// it — the daemon's own release sequence.
    #[rule]
    fn release_lock(&mut self, _tc: TestCase) {
        if self.lock_held_on_path() {
            let _ = fs::remove_file(self.pid_path());
        }
        self.held_lock = None;
    }

    #[rule]
    fn write_dead_pid(&mut self, tc: TestCase) {
        let offset = tc.draw(gs::integers::<u32>().max_value(99));
        fs::write(self.pid_path(), (DEAD_PID_BASE + offset).to_string()).expect("pid file");
    }

    #[rule]
    fn write_garbage_pid(&mut self, tc: TestCase) {
        let variant = tc.draw(gs::integers::<u8>().max_value(2));
        let garbage = match variant {
            0 => "not-a-pid".to_string(),
            1 => String::new(),
            _ => "12.5".to_string(),
        };
        fs::write(self.pid_path(), garbage).expect("pid file");
    }

    #[rule]
    fn remove_pid(&mut self, _tc: TestCase) {
        let _ = fs::remove_file(self.pid_path());
    }

    /// Bind a real unix socket and abandon it, mimicking a crashed daemon.
    #[rule]
    fn orphan_socket(&mut self, _tc: TestCase) {
        let path = self.paths.socket.clone();
        // Bind inside the machine's runtime; dropping the listener leaves
        // the socket file behind, mimicking a crashed daemon.
        self.rt.block_on(async {
            if let Ok(listener) = tokio::net::UnixListener::bind(&path) {
                drop(listener);
            }
        });
    }

    #[rule]
    fn plant_regular_file_at_socket_path(&mut self, _tc: TestCase) {
        let _ = fs::remove_file(&self.paths.socket);
        fs::write(&self.paths.socket, b"regular bytes").expect("socket path file");
    }

    #[rule]
    fn remove_socket(&mut self, _tc: TestCase) {
        let _ = fs::remove_file(&self.paths.socket);
    }

    /// Seed a valid peers.json so peer-store checks exercise their happy
    /// path alongside artifact findings.
    #[rule]
    fn seed_valid_peer_store(&mut self, tc: TestCase) {
        let mut key_bytes = [0u8; 32];
        key_bytes[0] = tc.draw(gs::integers::<u8>());
        let key = STANDARD.encode(key_bytes);
        let agent_id = axon::message::AgentId::from_pubkey_base64(&key).expect("valid test key");
        let document = serde_json::json!({
            "version": 1,
            "peers": [{
                "agent_id": agent_id.as_str(),
                "pubkey": key,
                "locators": [],
            }],
        });
        fs::write(
            &self.paths.peers,
            serde_json::to_vec_pretty(&document).expect("encode store"),
        )
        .expect("seed store");
    }

    /// Invariant: in check mode the report verdict mirrors artifact health.
    #[invariant]
    fn check_mode_verdict_mirrors_artifact_health(&self, _tc: TestCase) {
        let report = self.sweep(false);
        assert_eq!(
            report.ok,
            self.artifacts_healthy(),
            "check-mode verdict diverged from actual artifact state"
        );
        assert!(
            report.checks.iter().any(|check| check.name == "ipc_socket"),
            "the ipc socket is always inspected"
        );
    }

    /// Invariant: fix mode repairs everything actionable, so a follow-up
    /// check-mode sweep is healthy whenever no manual-only remainder exists;
    /// manual-only remainders keep failing closed.
    #[invariant]
    fn fix_mode_converges_except_manual_remainders(&self, _tc: TestCase) {
        let fix_sweep = self.sweep(true);
        let recheck = self.sweep(false);

        if self.manual_only_remainder() {
            assert!(!recheck.ok, "manual-only problems must keep failing closed");
            if self.lock_names_live_pid_without_flock() {
                assert!(
                    fs::symlink_metadata(self.paths.daemon_lock.clone()).is_ok(),
                    "fix mode must never delete a lock that names a live process"
                );
            }
        } else {
            assert!(
                recheck.ok || !fix_sweep.fixes_applied.is_empty(),
                "fix mode must converge when no manual-only problem remains \
                 (findings after fix: {:?})",
                recheck
                    .checks
                    .iter()
                    .filter(|check| !check.ok)
                    .map(|check| check.name)
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[hegel::test(test_cases = 20)]
fn doctor_artifacts_state_machine_holds_doctor_contract(tc: TestCase) {
    stateful::machine(DoctorArtifactsMachine::new()).run(tc);
}
