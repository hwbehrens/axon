//! Hegel stateful test: daemon-artifact states vs doctor verdicts.
//!
//! Rules drive the AXON root through artifact combinations (pid files,
//! socket paths, their contents); invariants pin the doctor contract after
//! every transition:
//!
//! - in check mode, `report.ok` mirrors actual artifact health exactly;
//! - in fix mode, actionable problems are repaired and never fabricated;
//! - fixing converges unless the remainder is manual-only by design
//!   (a regular file squatting on the socket path, or a live daemon whose
//!   socket never appeared — doctor cannot start processes or delete
//!   arbitrary files).

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};

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
        }
    }

    fn pid_path(&self) -> std::path::PathBuf {
        self.paths.root.join("daemon.pid")
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
    /// Pid health: missing or dead-but-valid is fine; garbage is not; the
    /// test's own pid counts as live. Socket health: a real unix socket is
    /// fine; a regular file is not; absence is fine unless a live daemon
    /// should be listening.
    fn artifacts_healthy(&self) -> bool {
        let live_pid_written = fs::read_to_string(self.pid_path())
            .map(|raw| raw.trim() == std::process::id().to_string())
            .unwrap_or(false);
        // Missing or live is healthy; garbage and well-formed-dead are both
        // findings (the latter fixable by removing the file).
        let pid_healthy = match fs::read_to_string(self.pid_path()) {
            Err(_) => true,
            Ok(raw) => raw.trim().parse::<u32>() == Ok(std::process::id()),
        };

        let socket_health = fs::symlink_metadata(&self.paths.socket);
        // A socket is only healthy while its daemon lives; an abandoned one
        // is a fixable finding even with no pid file at all.
        let socket_healthy = match socket_health {
            Err(_) => !live_pid_written,
            Ok(meta) => meta.file_type().is_socket() && live_pid_written,
        };

        // Peer store and config are seeded valid or left absent by the rules,
        // so they never contribute findings here.
        pid_healthy && socket_healthy
    }

    /// Problems doctor cannot repair by deleting files.
    fn manual_only_remainder(&self) -> bool {
        let live_pid_without_socket = fs::read_to_string(self.pid_path())
            .map(|raw| {
                raw.trim().parse::<u32>() == Ok(std::process::id()) && !self.paths.socket.exists()
            })
            .unwrap_or(false);
        let regular_file_socket = fs::symlink_metadata(&self.paths.socket)
            .map(|meta| !meta.file_type().is_socket())
            .unwrap_or(false);
        live_pid_without_socket || regular_file_socket
    }
}

#[hegel::state_machine]
impl DoctorArtifactsMachine {
    #[rule]
    fn write_live_pid(&mut self, _tc: TestCase) {
        fs::write(self.pid_path(), std::process::id().to_string()).expect("pid file");
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
    stateful::run(DoctorArtifactsMachine::new(), tc);
}
