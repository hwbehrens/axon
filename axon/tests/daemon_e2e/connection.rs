use super::*;

/// Wrong pubkey in static peer config prevents QUIC connection and
/// returns an instructive error through IPC send.
#[tokio::test]

async fn wrong_pubkey_prevents_connection() {
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();

    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    paths_a.ensure_root_exists().unwrap();
    let id_a = Identity::load_or_generate(&paths_a).unwrap();

    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    paths_b.ensure_root_exists().unwrap();
    let id_b = Identity::load_or_generate(&paths_b).unwrap();

    let port_a = pick_free_port();
    let port_b = pick_free_port();

    // A has the correct address for B but a WRONG pubkey.
    let wrong_pubkey = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".to_string();
    let peers_for_a = vec![StaticPeerConfig {
        agent_id: id_b.agent_id().into(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: wrong_pubkey,
    }];
    // B has correct config for A so it can start normally.
    let peers_for_b = vec![StaticPeerConfig {
        agent_id: id_a.agent_id().into(),
        addr: format!("127.0.0.1:{port_a}").parse().unwrap(),
        pubkey: id_a.public_key_base64().to_string(),
    }];

    let daemon_a = spawn_daemon(dir_a.path(), port_a, peers_for_a);
    let daemon_b = spawn_daemon(dir_b.path(), port_b, peers_for_b);

    assert!(wait_for_socket(&daemon_a.paths, Duration::from_secs(5)).await);
    assert!(wait_for_socket(&daemon_b.paths, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Attempt send from A → B. Should fail due to pubkey mismatch.
    let send_cmd = json!({
        "cmd": "send",
        "to": id_b.agent_id(),
        "kind": "request",
        "payload": {}
    });
    let reply = ipc_command(&daemon_a.paths.socket, send_cmd).await.unwrap();
    assert_eq!(
        reply["ok"],
        json!(false),
        "send with wrong pubkey should fail"
    );
    assert!(
        reply["error"].as_str().is_some(),
        "error message should be present"
    );

    // Peers command should show B as not connected.
    let peers = ipc_command(&daemon_a.paths.socket, json!({"cmd": "peers"}))
        .await
        .unwrap();
    let peer_list = peers["peers"].as_array().unwrap();
    let b_entry = peer_list
        .iter()
        .find(|p| p["agent_id"].as_str() == Some(id_b.agent_id()));
    if let Some(entry) = b_entry {
        assert_ne!(
            entry["status"].as_str(),
            Some("connected"),
            "B should not be connected with wrong pubkey"
        );
    }

    daemon_a.shutdown().await;
    daemon_b.shutdown().await;
}

/// Send to unreachable peer: when a peer is known but unreachable,
/// request send should fail with timeout or peer_unreachable.
#[tokio::test]

async fn send_to_unreachable_peer_returns_error() {
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();

    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    paths_a.ensure_root_exists().unwrap();
    Identity::load_or_generate(&paths_a).unwrap();

    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    paths_b.ensure_root_exists().unwrap();
    let id_b = Identity::load_or_generate(&paths_b).unwrap();

    let port_a = pick_free_port();
    let port_b = pick_free_port();

    // Start ONLY daemon A with a static peer entry for B (which is not running).
    let peers_for_a = vec![StaticPeerConfig {
        agent_id: id_b.agent_id().into(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: id_b.public_key_base64().to_string(),
    }];
    let daemon_a = spawn_daemon(dir_a.path(), port_a, peers_for_a);
    assert!(wait_for_socket(&daemon_a.paths, Duration::from_secs(5)).await);

    // B is NOT running. Bound request timeout to make this deterministic.
    let reply = ipc_command_timeout(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": id_b.agent_id(),
            "kind": "request",
            "timeout_secs": 2,
            "payload": {}
        }),
        Duration::from_secs(10),
    )
    .await
    .unwrap();

    assert_eq!(reply["ok"], json!(false));
    let error = reply["error"].as_str().unwrap();
    assert!(
        matches!(error, "timeout" | "peer_unreachable"),
        "error should be timeout or peer_unreachable, got: {error}"
    );

    // Status counters should not increment.
    let status = ipc_command(&daemon_a.paths.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    assert_eq!(
        status["messages_sent"].as_u64().unwrap(),
        0,
        "messages_sent should not increment on failed send"
    );

    daemon_a.shutdown().await;
}

/// add_peer IPC command enrolls a peer at runtime without daemon restart.
#[tokio::test]
async fn add_peer_hotloads_into_peer_table() {
    let daemon_root = tempdir().unwrap();
    let peer_root = tempdir().unwrap();

    let daemon_paths = AxonPaths::from_root(PathBuf::from(daemon_root.path()));
    daemon_paths.ensure_root_exists().unwrap();
    let daemon_id = Identity::load_or_generate(&daemon_paths).unwrap();

    let peer_paths = AxonPaths::from_root(PathBuf::from(peer_root.path()));
    peer_paths.ensure_root_exists().unwrap();
    let peer_id = Identity::load_or_generate(&peer_paths).unwrap();

    let daemon = spawn_daemon(daemon_root.path(), pick_free_port(), vec![]);
    assert!(wait_for_socket(&daemon.paths, Duration::from_secs(5)).await);

    let add_reply = ipc_command(
        &daemon.paths.socket,
        json!({
            "cmd": "add_peer",
            "pubkey": peer_id.public_key_base64(),
            "addr": format!("127.0.0.1:{}", pick_free_port())
        }),
    )
    .await
    .unwrap();
    assert_eq!(add_reply["ok"], json!(true));
    assert_eq!(add_reply["agent_id"], json!(peer_id.agent_id()));

    let peers = ipc_command(&daemon.paths.socket, json!({"cmd": "peers"}))
        .await
        .unwrap();
    let list = peers["peers"].as_array().unwrap();
    let found = list
        .iter()
        .find(|entry| entry["agent_id"].as_str() == Some(peer_id.agent_id()))
        .expect("added peer should appear in peers list");
    assert_eq!(found["source"], json!("static"));

    // Sanity: local daemon identity should not equal added peer identity.
    assert_ne!(daemon_id.agent_id(), peer_id.agent_id());

    daemon.shutdown().await;
}

/// Shutdown during active reconnect churn: daemon has a static peer that
/// is unreachable, so the reconnect loop is actively retrying. Cancel
/// should still shut down cleanly within a bounded time.
#[tokio::test]

async fn shutdown_during_reconnect_churn() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();

    // Configure a peer that doesn't exist — reconnect loop will churn.
    let peers = vec![StaticPeerConfig {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: "127.0.0.1:1".parse().unwrap(), // unreachable
        pubkey: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".to_string(),
    }];
    let daemon = spawn_daemon(dir.path(), port, peers);
    assert!(wait_for_socket(&daemon.paths, Duration::from_secs(5)).await);

    // Let the reconnect loop churn for a few cycles.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Shutdown should complete within bounded time even while reconnecting.
    let start = tokio::time::Instant::now();
    daemon.cancel.cancel();
    let result = timeout(Duration::from_secs(10), daemon.handle)
        .await
        .expect("daemon did not shut down in time")
        .expect("daemon task panicked");
    assert!(result.is_ok(), "daemon should shut down cleanly");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(8),
        "shutdown should complete in bounded time, took {:?}",
        elapsed
    );
}

/// IPC remains responsive while reconnect loop is churning against
/// an unreachable peer. Verifies that spawned reconnect tasks don't
/// block the main event loop.
#[tokio::test]

async fn ipc_responsive_during_reconnect_churn() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();

    // Configure a peer that doesn't exist — reconnect loop will churn.
    let peers = vec![StaticPeerConfig {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: "127.0.0.1:1".parse().unwrap(),
        pubkey: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".to_string(),
    }];
    let daemon = spawn_daemon(dir.path(), port, peers);
    assert!(wait_for_socket(&daemon.paths, Duration::from_secs(5)).await);

    // Let the reconnect loop start churning.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // IPC commands should still respond promptly (< 2s) even though
    // the reconnect loop is actively trying to connect to an unreachable peer.
    let start = tokio::time::Instant::now();
    let status = ipc_command(&daemon.paths.socket, json!({"cmd": "status"}))
        .await
        .expect("status command should succeed during reconnect churn");
    let elapsed = start.elapsed();

    assert_eq!(status["ok"], json!(true));
    assert!(
        elapsed < Duration::from_secs(2),
        "IPC should respond within 2s during reconnect churn, took {:?}",
        elapsed
    );

    // Peers command should also respond promptly.
    let start = tokio::time::Instant::now();
    let peers_resp = ipc_command(&daemon.paths.socket, json!({"cmd": "peers"}))
        .await
        .expect("peers command should succeed during reconnect churn");
    let elapsed = start.elapsed();

    assert_eq!(peers_resp["ok"], json!(true));
    assert!(
        elapsed < Duration::from_secs(2),
        "peers should respond within 2s during reconnect churn, took {:?}",
        elapsed
    );

    daemon.shutdown().await;
}

/// Shutdown while IPC clients are connected: daemon exits cleanly
/// and the peer daemon remains functional.
#[tokio::test]

async fn shutdown_with_connected_ipc_clients() {
    let TwoDaemons {
        daemon_a, daemon_b, ..
    } = setup_connected_pair().await;

    // Connect an IPC client to B.
    let _client = UnixStream::connect(&daemon_b.paths.socket).await.unwrap();

    let b_socket_path = daemon_b.paths.socket.clone();

    // Shut down B while the client is connected.
    daemon_b.cancel.cancel();
    let result = timeout(Duration::from_secs(10), daemon_b.handle)
        .await
        .expect("daemon B did not shut down in time")
        .expect("daemon B task panicked");
    assert!(result.is_ok(), "daemon B should exit cleanly: {:?}", result);

    // Socket should be cleaned up.
    assert!(
        !b_socket_path.exists(),
        "socket should be removed after shutdown"
    );

    // Daemon A should still be functional.
    let status = ipc_command(&daemon_a.paths.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    assert_eq!(status["ok"], json!(true));

    daemon_a.shutdown().await;
}
