//! Daemon lifecycle integration tests.
//!
//! These tests exercise full daemon startup, shutdown, and reconnection
//! behaviors. They spin up real daemon instances in spawned tasks with
//! temp directories and CancellationTokens for controlled lifecycle.
//!
//! Marked `#[ignore]` because they are longer-running e2e tests.

use std::path::PathBuf;
use std::time::Duration;

use axon::config::{AxonPaths, StaticPeerConfig};
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

// =========================================================================
// Helpers
// =========================================================================

/// Bind a UDP socket to port 0 and return the OS-assigned port.
fn pick_free_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.local_addr().unwrap().port()
}

/// Start a daemon in a background task, returning its cancel token and paths.
fn spawn_daemon(
    dir: &std::path::Path,
    port: u16,
    enable_mdns: bool,
    peers: Vec<StaticPeerConfig>,
    agent_id_override: Option<String>,
) -> (CancellationToken, AxonPaths, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let cancel = CancellationToken::new();
    let paths = AxonPaths::from_root(PathBuf::from(dir));
    paths.ensure_root_exists().unwrap();

    // Write static peer config if any.
    if !peers.is_empty() {
        let config = axon::config::Config {
            port: Some(port),
            peers,
        };
        let toml = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&paths.config, toml).unwrap();
    }

    let opts = DaemonOptions {
        port: Some(port),
        enable_mdns,
        axon_root: Some(PathBuf::from(dir)),
        agent_id: agent_id_override,
        cancel: Some(cancel.clone()),
    };

    let handle = tokio::spawn(async move { run_daemon(opts).await });

    (cancel, paths, handle)
}

/// Wait until the IPC socket file appears, with a timeout.
async fn wait_for_socket(paths: &AxonPaths, timeout_dur: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout_dur;
    loop {
        if paths.socket.exists() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Send a JSON command over IPC and read one response line.
async fn ipc_command(socket_path: &std::path::Path, command: Value) -> anyhow::Result<Value> {
    let mut stream = UnixStream::connect(socket_path).await?;
    let line = serde_json::to_string(&command)?;
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    let bytes = timeout(Duration::from_secs(5), reader.read_line(&mut response)).await??;
    if bytes == 0 {
        anyhow::bail!("daemon closed connection");
    }
    Ok(serde_json::from_str(response.trim())?)
}

// =========================================================================
// Tests
// =========================================================================

/// Graceful shutdown: start a daemon, cancel it, verify IPC socket removed
/// and known_peers.json saved.
#[tokio::test]
#[ignore]
async fn graceful_shutdown_cleans_up() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();
    let (cancel, paths, handle) = spawn_daemon(dir.path(), port, false, vec![], None);

    // Wait for daemon to be ready.
    assert!(
        wait_for_socket(&paths, Duration::from_secs(5)).await,
        "daemon socket did not appear"
    );

    // Verify daemon responds to status.
    let status = ipc_command(&paths.socket, json!({"cmd": "status"}))
        .await
        .expect("status command failed");
    assert_eq!(status["ok"], json!(true));

    // Signal shutdown.
    cancel.cancel();

    // Wait for the daemon task to finish.
    let result = timeout(Duration::from_secs(10), handle)
        .await
        .expect("daemon did not shut down in time")
        .expect("daemon task panicked");
    assert!(result.is_ok(), "daemon returned error: {:?}", result);

    // Socket file should be cleaned up.
    assert!(
        !paths.socket.exists(),
        "IPC socket was not removed after shutdown"
    );

    // known_peers.json should exist (saved during shutdown).
    assert!(
        paths.known_peers.exists(),
        "known_peers.json was not saved during shutdown"
    );
}

/// Reconnect after peer restart: start two daemons with static peer config,
/// verify they connect, shut one down, then restart it and verify reconnection.
#[tokio::test]
#[ignore]
async fn reconnect_after_peer_restart() {
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();

    // Generate identities first so we know their agent_ids for static config.
    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    paths_a.ensure_root_exists().unwrap();
    let id_a = Identity::load_or_generate(&paths_a).unwrap();

    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    paths_b.ensure_root_exists().unwrap();
    let id_b = Identity::load_or_generate(&paths_b).unwrap();

    let port_a = pick_free_port();
    let port_b = pick_free_port();

    let peers_for_a = vec![StaticPeerConfig {
        agent_id: id_b.agent_id().to_string(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: id_b.public_key_base64().to_string(),
    }];
    let peers_for_b = vec![StaticPeerConfig {
        agent_id: id_a.agent_id().to_string(),
        addr: format!("127.0.0.1:{port_a}").parse().unwrap(),
        pubkey: id_a.public_key_base64().to_string(),
    }];

    // Start both daemons.
    let (cancel_a, paths_a, handle_a) =
        spawn_daemon(dir_a.path(), port_a, false, peers_for_a.clone(), None);
    let (cancel_b, paths_b, handle_b) =
        spawn_daemon(dir_b.path(), port_b, false, peers_for_b.clone(), None);

    assert!(
        wait_for_socket(&paths_a, Duration::from_secs(5)).await,
        "daemon A socket did not appear"
    );
    assert!(
        wait_for_socket(&paths_b, Duration::from_secs(5)).await,
        "daemon B socket did not appear"
    );

    // Allow time for initial connection + hello handshake.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify both daemons see each other as peers.
    let peers_resp = ipc_command(&paths_a.socket, json!({"cmd": "peers"}))
        .await
        .expect("peers command failed");
    assert_eq!(peers_resp["ok"], json!(true));
    let peer_list = peers_resp["peers"].as_array().expect("peers is array");
    assert!(
        !peer_list.is_empty(),
        "daemon A should have at least one peer"
    );

    // Shut down daemon B.
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(10), handle_b).await;

    // Wait for A to notice B is disconnected.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Restart daemon B on the same port.
    let (cancel_b2, paths_b2, handle_b2) =
        spawn_daemon(dir_b.path(), port_b, false, peers_for_b, None);
    assert!(
        wait_for_socket(&paths_b2, Duration::from_secs(5)).await,
        "daemon B2 socket did not appear"
    );

    // Allow time for reconnection (backoff window).
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Verify A has reconnected to B.
    let peers_resp2 = ipc_command(&paths_a.socket, json!({"cmd": "peers"}))
        .await
        .expect("peers command failed after restart");
    let peer_list2 = peers_resp2["peers"].as_array().expect("peers is array");
    let b_peer = peer_list2
        .iter()
        .find(|p| p["id"].as_str() == Some(id_b.agent_id()))
        .expect("daemon B not found in peers after restart");
    assert_eq!(
        b_peer["status"].as_str(),
        Some("connected"),
        "daemon B should be connected after restart"
    );

    // Clean up.
    cancel_a.cancel();
    cancel_b2.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
    let _ = timeout(Duration::from_secs(5), handle_b2).await;
}

/// Initiator rule: the daemon with the lower agent_id initiates.
/// Start two daemons and verify the lower-ID one establishes the connection.
#[tokio::test]
#[ignore]
async fn initiator_rule_lower_id_connects() {
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();

    // Generate identities and determine which is lower.
    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    paths_a.ensure_root_exists().unwrap();
    let id_a = Identity::load_or_generate(&paths_a).unwrap();

    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    paths_b.ensure_root_exists().unwrap();
    let id_b = Identity::load_or_generate(&paths_b).unwrap();

    let (_lower_id, _higher_id) = if id_a.agent_id() < id_b.agent_id() {
        (id_a.agent_id().to_string(), id_b.agent_id().to_string())
    } else {
        (id_b.agent_id().to_string(), id_a.agent_id().to_string())
    };

    let port_a = pick_free_port();
    let port_b = pick_free_port();

    let peers_for_a = vec![StaticPeerConfig {
        agent_id: id_b.agent_id().to_string(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: id_b.public_key_base64().to_string(),
    }];
    let peers_for_b = vec![StaticPeerConfig {
        agent_id: id_a.agent_id().to_string(),
        addr: format!("127.0.0.1:{port_a}").parse().unwrap(),
        pubkey: id_a.public_key_base64().to_string(),
    }];

    let (cancel_a, paths_a, handle_a) =
        spawn_daemon(dir_a.path(), port_a, false, peers_for_a, None);
    let (cancel_b, paths_b, handle_b) =
        spawn_daemon(dir_b.path(), port_b, false, peers_for_b, None);

    assert!(
        wait_for_socket(&paths_a, Duration::from_secs(5)).await,
        "daemon A socket did not appear"
    );
    assert!(
        wait_for_socket(&paths_b, Duration::from_secs(5)).await,
        "daemon B socket did not appear"
    );

    // Allow time for the lower-ID daemon to connect.
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Both should see each other. The lower-ID daemon should have initiated.
    let peers_a = ipc_command(&paths_a.socket, json!({"cmd": "peers"}))
        .await
        .expect("peers from A");
    let peers_b = ipc_command(&paths_b.socket, json!({"cmd": "peers"}))
        .await
        .expect("peers from B");

    let list_a = peers_a["peers"].as_array().unwrap();
    let list_b = peers_b["peers"].as_array().unwrap();

    let a_sees_b = list_a.iter().any(|p| {
        p["id"].as_str() == Some(id_b.agent_id())
            && p["status"].as_str() == Some("connected")
    });
    let b_sees_a = list_b.iter().any(|p| {
        p["id"].as_str() == Some(id_a.agent_id())
            && p["status"].as_str() == Some("connected")
    });

    assert!(a_sees_b, "daemon A should see daemon B as connected");
    assert!(b_sees_a, "daemon B should see daemon A as connected");

    // Clean up.
    cancel_a.cancel();
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
    let _ = timeout(Duration::from_secs(5), handle_b).await;
}
