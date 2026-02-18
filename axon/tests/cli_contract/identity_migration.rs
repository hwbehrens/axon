use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::fs;
use tempfile::tempdir;

#[test]
fn legacy_raw_identity_key_is_auto_migrated() {
    let bin = super::axon_bin();
    let root = tempdir().expect("tempdir");
    fs::create_dir_all(root.path()).expect("create root");
    fs::write(root.path().join("identity.key"), [7u8; 32]).expect("write raw key");

    let output = super::run_command(std::process::Command::new(&bin).args([
        "--state-root",
        root.path().to_str().expect("utf8 path"),
        "identity",
    ]));
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Your enrollment token"));
    assert!(stdout.lines().any(|line| line.starts_with("axon://")));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(
        "Notice: migrated identity.key from legacy raw format to base64. Agent ID unchanged."
    ));

    let migrated = fs::read_to_string(root.path().join("identity.key")).expect("read migrated key");
    let decoded = STANDARD
        .decode(migrated.trim())
        .expect("migrated identity.key should be base64");
    assert_eq!(decoded.len(), 32);
}
