use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use tempfile::tempdir;

fn axon_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("CARGO_BIN_EXE_axon") {
        return PathBuf::from(bin);
    }

    let current = std::env::current_exe().expect("resolve current test executable");
    let debug_dir = current
        .parent()
        .and_then(Path::parent)
        .expect("resolve target debug dir");
    let fallback = if cfg!(windows) {
        debug_dir.join("axon.exe")
    } else {
        debug_dir.join("axon")
    };
    assert!(
        fallback.exists(),
        "failed to locate axon binary via CARGO_BIN_EXE_axon and fallback path {}",
        fallback.display()
    );
    fallback
}

fn run_command(cmd: &mut Command) -> Output {
    cmd.output().expect("failed to execute axon binary")
}

fn run_doctor(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(axon_bin());
    cmd.arg("--state-root")
        .arg(root.to_str().expect("utf8 path"))
        .arg("doctor");
    cmd.args(args);
    run_command(&mut cmd)
}

fn run_doctor_json(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(axon_bin());
    cmd.arg("--state-root")
        .arg(root.to_str().expect("utf8 path"))
        .arg("doctor")
        .arg("--json");
    cmd.args(args);
    run_command(&mut cmd)
}

fn parse_report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("doctor stdout should be valid JSON")
}

fn check_by_name<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .expect("checks must be array")
        .iter()
        .find(|check| check.get("name") == Some(&Value::String(name.to_string())))
        .unwrap_or_else(|| panic!("missing check '{name}'"))
}

#[test]
fn doctor_check_mode_reports_missing_identity_without_creating_files() {
    let root = tempdir().expect("tempdir");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("set perms");

    let output = run_doctor_json(root.path(), &[]);
    assert_eq!(output.status.code(), Some(2));

    let report = parse_report(&output);
    assert_eq!(report["mode"], "check");
    assert_eq!(report["ok"], false);

    let identity = check_by_name(&report, "identity");
    assert_eq!(identity["ok"], false);
    assert_eq!(identity["fixable"], true);
    assert!(
        identity["message"]
            .as_str()
            .expect("identity message")
            .contains("doctor --fix")
    );

    assert!(
        !root.path().join("identity.key").exists(),
        "check mode should not generate identity files"
    );
}

#[test]
fn doctor_fix_mode_generates_identity() {
    let root = tempdir().expect("tempdir");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("set perms");

    let output = run_doctor_json(root.path(), &["--fix"]);
    assert!(output.status.success());

    let report = parse_report(&output);
    assert_eq!(report["mode"], "fix");
    assert_eq!(report["ok"], true);

    let identity = check_by_name(&report, "identity");
    assert_eq!(identity["ok"], true);

    let key_contents = fs::read_to_string(root.path().join("identity.key")).expect("read key");
    let decoded = STANDARD
        .decode(key_contents.trim())
        .expect("identity.key should be base64");
    assert_eq!(decoded.len(), 32);
}

#[test]
fn doctor_fix_requires_rekey_for_unrecoverable_identity() {
    let root = tempdir().expect("tempdir");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("set perms");
    fs::write(root.path().join("identity.key"), "not base64 at all").expect("write invalid key");

    let output = run_doctor_json(root.path(), &["--fix"]);
    assert_eq!(output.status.code(), Some(2));

    let report = parse_report(&output);
    assert_eq!(report["mode"], "fix");
    assert_eq!(report["ok"], false);

    let identity = check_by_name(&report, "identity");
    assert_eq!(identity["ok"], false);
    assert!(
        identity["message"]
            .as_str()
            .expect("identity message")
            .contains("--rekey")
    );

    let still_invalid = fs::read_to_string(root.path().join("identity.key")).expect("read key");
    assert_eq!(still_invalid, "not base64 at all");
}

#[test]
fn doctor_fix_rekey_backs_up_and_regenerates_identity() {
    let root = tempdir().expect("tempdir");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("set perms");
    fs::write(root.path().join("identity.key"), "not base64 at all").expect("write invalid key");

    let output = run_doctor_json(root.path(), &["--fix", "--rekey"]);
    assert!(output.status.success());

    let report = parse_report(&output);
    assert_eq!(report["mode"], "fix");
    assert_eq!(report["ok"], true);

    let identity = check_by_name(&report, "identity");
    assert_eq!(identity["ok"], true);

    let key_contents = fs::read_to_string(root.path().join("identity.key")).expect("read key");
    let decoded = STANDARD
        .decode(key_contents.trim())
        .expect("identity.key should be base64");
    assert_eq!(decoded.len(), 32);

    let mut backup_found = false;
    for entry in fs::read_dir(root.path()).expect("read dir") {
        let path = entry.expect("dir entry").path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("identity.key.bak.") {
            backup_found = true;
            break;
        }
    }
    assert!(backup_found, "expected identity.key backup file");
}

#[test]
fn doctor_subcommand_is_visible_in_help() {
    let output = run_command(Command::new(axon_bin()).arg("--help"));
    assert!(output.status.success());

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("  doctor"));
}

#[test]
fn doctor_default_output_is_human_readable() {
    let root = tempdir().expect("tempdir");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("set perms");

    let output = run_doctor(root.path(), &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Doctor:"));
    assert!(!stdout.trim_start().starts_with('{'));
}
