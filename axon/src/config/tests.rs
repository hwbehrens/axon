use tempfile::tempdir;

use super::*;

#[tokio::test]
async fn missing_config_uses_local_defaults() {
    let root = tempdir().expect("tempdir");

    let config = Config::load(&root.path().join("config.yaml"))
        .await
        .expect("load missing config");

    assert_eq!(config, Config::default());
    assert_eq!(config.effective_port(None), 7100);
}

#[tokio::test]
async fn config_contains_only_local_daemon_settings() {
    let root = tempdir().expect("tempdir");
    let path = root.path().join("config.yaml");
    tokio::fs::write(
        &path,
        "name: alice\nport: 7200\nadvertise_addr: alice.local:7200\n",
    )
    .await
    .expect("write config");

    let config = Config::load(&path).await.expect("load config");

    assert_eq!(config.name.as_deref(), Some("alice"));
    assert_eq!(config.port, Some(7200));
    assert_eq!(config.advertise_addr.as_deref(), Some("alice.local:7200"));
}

#[tokio::test]
async fn legacy_peer_entries_are_rejected_instead_of_imported() {
    let root = tempdir().expect("tempdir");
    let path = root.path().join("config.yaml");
    tokio::fs::write(
        &path,
        "peers:\n  - agent_id: ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n    addr: 127.0.0.1:7100\n    pubkey: invalid\n",
    )
    .await
    .expect("write config");

    let error = Config::load(&path)
        .await
        .expect_err("legacy peers must fail closed");

    assert!(error.to_string().contains("Legacy `peers` entries"));
}

#[tokio::test]
async fn persisted_config_roundtrips() {
    let root = tempdir().expect("tempdir");
    let path = root.path().join("config.yaml");
    let expected = PersistedConfig {
        name: Some("alice".to_string()),
        port: Some(7200),
        advertise_addr: Some("alice.local:7200".to_string()),
    };

    save_persisted_config(&path, &expected)
        .await
        .expect("save config");
    let loaded = load_persisted_config(&path).await.expect("load config");

    assert_eq!(loaded, expected);
}

#[test]
fn paths_separate_peer_store_from_legacy_cache() {
    let root = std::path::PathBuf::from("/tmp/axon-path-test");
    let paths = AxonPaths::from_root(root.clone());

    assert_eq!(paths.peers, root.join("peers.json"));
    assert_eq!(paths.legacy_known_peers, root.join("known_peers.json"));
}

#[test]
fn legacy_known_peers_file_is_reported() {
    let root = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(root.path().to_path_buf());
    std::fs::write(&paths.legacy_known_peers, b"[]").expect("write legacy file");

    let error = paths
        .reject_legacy_peer_state()
        .expect_err("legacy peer state must be rejected");

    assert!(error.to_string().contains("intentionally re-enroll"));
}

#[tokio::test]
async fn unreadable_config_fails_closed_instead_of_using_defaults() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().expect("tempdir");
    let path = root.path().join("config.yaml");
    fs::write(&path, "name: alice\n").expect("seed config");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
    // Root (and some sandboxes) bypass permission bits; skip when the
    // property cannot be exercised.
    if fs::read(&path).is_ok() {
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
        return;
    }

    let result = load_persisted_config(&path).await;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));

    assert!(
        result.is_err(),
        "an unreadable config must fail closed, not silently use defaults"
    );
}

#[test]
fn ensure_root_exists_creates_and_secures_the_root() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().expect("tempdir");
    let nested = root.path().join("axon-state");
    let paths = AxonPaths::from_root(nested.clone());

    paths.ensure_root_exists().expect("create missing root");
    assert!(nested.is_dir());
    let mode = fs::symlink_metadata(&nested).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "AXON root must be owner-only");

    // Idempotent on an existing directory.
    paths
        .ensure_root_exists()
        .expect("existing root stays acceptable");
}

#[test]
fn ensure_root_exists_rejects_symlinked_root() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().expect("tempdir");
    let real = root.path().join("real");
    std::fs::create_dir_all(&real).expect("real dir");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    AxonPaths::from_root(link.clone())
        .ensure_root_exists()
        .expect_err("symlinked AXON root must be rejected");
}

#[test]
fn discover_with_override_ignores_blank_axon_root_env() {
    // Env mutation must be serialized against other env-touching tests.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap();

    // SAFETY: single-threaded access to this process env var is guaranteed
    // by ENV_LOCK; no other test reads AXON_ROOT concurrently.
    let previous = std::env::var("AXON_ROOT").ok();
    unsafe { std::env::set_var("AXON_ROOT", "   \t") };
    let discovered = AxonPaths::discover_with_override(None).expect("discover falls through");
    match previous {
        Some(value) => unsafe { std::env::set_var("AXON_ROOT", value) },
        None => unsafe { std::env::remove_var("AXON_ROOT") },
    }

    assert_eq!(
        discovered.root,
        std::env::var("HOME")
            .map(|home| std::path::PathBuf::from(home).join(".axon"))
            .unwrap_or_default(),
        "a blank AXON_ROOT must fall through to HOME discovery"
    );

    let override_root = tempdir().expect("tempdir");
    let overridden =
        AxonPaths::discover_with_override(Some(override_root.path())).expect("override wins");
    assert_eq!(overridden.root, override_root.path());
}
