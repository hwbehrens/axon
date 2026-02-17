use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn config_defaults_when_missing() {
    let dir = tempdir().expect("temp dir");
    let cfg = Config::load(&dir.path().join("missing.toml"))
        .await
        .expect("load missing config");
    assert_eq!(cfg.effective_port(None), 7100);
    assert!(cfg.peers.is_empty());
}

#[tokio::test]
async fn config_parses_static_peers() {
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

    let cfg = Config::load(&path).await.expect("load config");
    assert_eq!(cfg.effective_port(None), 8111);
    assert_eq!(cfg.peers.len(), 1);
    assert_eq!(cfg.peers[0].addr.to_string(), "127.0.0.1:7100");
}

#[tokio::test]
async fn config_parses_hostname_peer_addr() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
                [[peers]]
                agent_id = "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                addr = "localhost:7100"
                pubkey = "Zm9v"
            "#,
    )
    .expect("write config");

    let cfg = Config::load(&path).await.expect("load config");
    assert_eq!(cfg.peers.len(), 1);
    assert_eq!(cfg.peers[0].addr.port(), 7100);
    assert!(cfg.peers[0].addr.ip().is_loopback());
}

#[tokio::test]
async fn config_skips_unresolvable_or_invalid_peer_addr() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
                [[peers]]
                agent_id = "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                addr = "127.0.0.1:7100"
                pubkey = "Zm9v"

                [[peers]]
                agent_id = "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                addr = "peer-does-not-exist.invalid:7100"
                pubkey = "YmFy"
            "#,
    )
    .expect("write config");

    let cfg = Config::load(&path).await.expect("load config");
    assert_eq!(cfg.peers.len(), 1);
    assert_eq!(
        cfg.peers[0].agent_id.as_str(),
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[test]
fn cli_override_takes_precedence() {
    let cfg = Config {
        name: None,
        port: Some(8000),
        peers: Vec::new(),
    };
    assert_eq!(cfg.effective_port(Some(9999)), 9999);
    assert_eq!(cfg.effective_port(None), 8000);
}

#[tokio::test]
async fn invalid_toml_returns_error() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "{{{{not toml!").expect("write");
    assert!(Config::load(&path).await.is_err());
}

#[tokio::test]
async fn config_ignores_unknown_fields() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "max_ipc_clients = 32\nmax_connections = 256\nkeepalive_secs = 5\nport = 7200\n",
    )
    .expect("write");
    let cfg = Config::load(&path)
        .await
        .expect("load config with old fields");
    assert_eq!(cfg.effective_port(None), 7200);
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
    let loaded = load_known_peers(&path).await.expect("load");
    assert_eq!(loaded, peers);
}

#[tokio::test]
async fn known_peers_empty_when_missing() {
    let dir = tempdir().expect("temp dir");
    let loaded = load_known_peers(&dir.path().join("missing.json"))
        .await
        .expect("load");
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
    assert_eq!(paths.socket, root.join("axon.sock"));
}

#[test]
fn discover_with_override_uses_override_root() {
    let root = PathBuf::from("/tmp/axon-override");
    let paths = AxonPaths::discover_with_override(Some(root.as_path())).expect("discover");
    assert_eq!(paths.root, root);
    assert_eq!(paths.socket, PathBuf::from("/tmp/axon-override/axon.sock"));
}

#[test]
fn peer_addr_parse_and_resolve_ipv4_socket() {
    let addr = PeerAddr::parse("127.0.0.1:7100").expect("parse");
    assert_eq!(
        addr.resolve().expect("resolve"),
        "127.0.0.1:7100".parse().expect("socket addr")
    );
}

#[test]
fn peer_addr_parse_hostname_with_port() {
    let addr = PeerAddr::parse("localhost:7100").expect("parse");
    let PeerAddr::Host { host, port } = addr else {
        panic!("expected host variant");
    };
    assert_eq!(host, "localhost");
    assert_eq!(port, 7100);
}

#[test]
fn peer_addr_requires_port() {
    let err = PeerAddr::parse("localhost").expect_err("missing port should fail");
    assert!(err.to_string().contains("host:port"));
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

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

proptest! {
    #[test]
    fn effective_port_cli_always_wins(config_port in proptest::option::of(1u16..),
                                      cli_port in 1u16..) {
        let cfg = Config {
            name: None,
            port: config_port,
            peers: Vec::new(),
        };
        prop_assert_eq!(cfg.effective_port(Some(cli_port)), cli_port);
    }

    #[test]
    fn effective_port_without_cli_uses_config_or_default(config_port in proptest::option::of(1u16..)) {
        let cfg = Config {
            name: None,
            port: config_port,
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
    let loaded = load_known_peers(&path).await.expect("load");
    assert_eq!(loaded, peers);
}
