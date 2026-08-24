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
