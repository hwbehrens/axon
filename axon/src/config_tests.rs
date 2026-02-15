use std::time::Duration;

use super::*;
use tempfile::tempdir;

#[test]
fn config_defaults_when_missing() {
    let dir = tempdir().expect("temp dir");
    let cfg = Config::load(&dir.path().join("missing.toml")).expect("load missing config");
    assert_eq!(cfg.effective_port(None), 7100);
    assert!(cfg.peers.is_empty());
}

#[test]
fn config_parses_static_peers() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
                port = 8111
                [[peers]]
                agent_id = "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                addr = "127.0.0.1:7100"
                pubkey = "Zm9v"
            "#,
    )
    .expect("write config");

    let cfg = Config::load(&path).expect("load config");
    assert_eq!(cfg.effective_port(None), 8111);
    assert_eq!(cfg.peers.len(), 1);
    assert_eq!(cfg.peers[0].addr.to_string(), "127.0.0.1:7100");
}

#[test]
fn cli_override_takes_precedence() {
    let cfg = Config {
        port: Some(8000),
        max_ipc_clients: None,
        max_connections: None,
        keepalive_secs: None,
        idle_timeout_secs: None,
        reconnect_max_backoff_secs: None,
        handshake_timeout_secs: None,
        inbound_read_timeout_secs: None,
        peers: Vec::new(),
    };
    assert_eq!(cfg.effective_port(Some(9999)), 9999);
    assert_eq!(cfg.effective_port(None), 8000);
}

#[test]
fn invalid_toml_returns_error() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "{{{{not toml!").expect("write");
    assert!(Config::load(&path).is_err());
}

#[tokio::test]
async fn known_peers_roundtrip() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("known.json");
    let peers = vec![KnownPeer {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: "127.0.0.1:7100".parse().expect("addr"),
        pubkey: "Zm9v".to_string(),
        last_seen_unix_ms: 123,
    }];

    save_known_peers(&path, &peers).await.expect("save");
    let loaded = load_known_peers(&path).expect("load");
    assert_eq!(loaded, peers);
}

#[test]
fn known_peers_empty_when_missing() {
    let dir = tempdir().expect("temp dir");
    let loaded = load_known_peers(&dir.path().join("missing.json")).expect("load");
    assert!(loaded.is_empty());
}

#[test]
fn discover_paths_from_root() {
    let root = PathBuf::from("/tmp/axon-test");
    let paths = AxonPaths::from_root(root.clone());
    assert_eq!(paths.identity_key, root.join("identity.key"));
    assert_eq!(paths.identity_pub, root.join("identity.pub"));
    assert_eq!(paths.config, root.join("config.toml"));
    assert_eq!(paths.known_peers, root.join("known_peers.json"));
    assert_eq!(paths.replay_cache, root.join("replay_cache.json"));
    assert_eq!(paths.socket, root.join("axon.sock"));
}

#[test]
fn ensure_root_creates_and_sets_perms() {
    let dir = tempdir().expect("temp dir");
    let root = dir.path().join("axon-subdir");
    let paths = AxonPaths::from_root(root.clone());
    paths.ensure_root_exists().expect("ensure root");
    assert!(root.exists());
    let mode = fs::metadata(&root).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn config_resource_limit_defaults() {
    let cfg = Config::default();
    assert_eq!(cfg.effective_max_ipc_clients(), 64);
    assert_eq!(cfg.effective_max_connections(), 128);
}

#[test]
fn config_transport_timing_defaults() {
    let cfg = Config::default();
    assert_eq!(cfg.effective_keepalive(), Duration::from_secs(15));
    assert_eq!(cfg.effective_idle_timeout(), Duration::from_secs(60));
    assert_eq!(
        cfg.effective_reconnect_max_backoff(),
        Duration::from_secs(30)
    );
}

#[test]
fn config_transport_timing_from_toml() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "keepalive_secs = 5\nidle_timeout_secs = 120\nreconnect_max_backoff_secs = 10\n",
    )
    .expect("write");
    let cfg = Config::load(&path).expect("load");
    assert_eq!(cfg.effective_keepalive(), Duration::from_secs(5));
    assert_eq!(cfg.effective_idle_timeout(), Duration::from_secs(120));
    assert_eq!(
        cfg.effective_reconnect_max_backoff(),
        Duration::from_secs(10)
    );
}

#[test]
fn config_resource_limits_from_toml() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "max_ipc_clients = 32\nmax_connections = 256\n").expect("write");
    let cfg = Config::load(&path).expect("load");
    assert_eq!(cfg.effective_max_ipc_clients(), 32);
    assert_eq!(cfg.effective_max_connections(), 256);
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

proptest! {
    #[test]
    fn effective_port_cli_always_wins(config_port in proptest::option::of(1u16..),
                                      cli_port in 1u16..) {
        let cfg = Config {
            port: config_port,
            max_ipc_clients: None,
            max_connections: None,
            keepalive_secs: None,
            idle_timeout_secs: None,
            reconnect_max_backoff_secs: None,
            handshake_timeout_secs: None,
            inbound_read_timeout_secs: None,
            peers: Vec::new(),
        };
        prop_assert_eq!(cfg.effective_port(Some(cli_port)), cli_port);
    }

    #[test]
    fn effective_port_without_cli_uses_config_or_default(config_port in proptest::option::of(1u16..)) {
        let cfg = Config {
            port: config_port,
            max_ipc_clients: None,
            max_connections: None,
            keepalive_secs: None,
            idle_timeout_secs: None,
            reconnect_max_backoff_secs: None,
            handshake_timeout_secs: None,
            inbound_read_timeout_secs: None,
            peers: Vec::new(),
        };
        let expected = config_port.unwrap_or(7100);
        prop_assert_eq!(cfg.effective_port(None), expected);
    }
}

// =========================================================================
// Mutation-coverage: save_known_peers creates parent dir
// =========================================================================

#[tokio::test]
async fn save_known_peers_creates_parent_dir() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("nested").join("subdir").join("known.json");
    let peers = vec![KnownPeer {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: "127.0.0.1:7100".parse().expect("addr"),
        pubkey: "Zm9v".to_string(),
        last_seen_unix_ms: 456,
    }];

    save_known_peers(&path, &peers)
        .await
        .expect("save should create parent dirs");
    assert!(path.exists(), "file should exist after save");
    let loaded = load_known_peers(&path).expect("load");
    assert_eq!(loaded, peers);
}
