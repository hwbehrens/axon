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
        "kind": "ping",
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
        .find(|p| p["id"].as_str() == Some(id_b.agent_id()));
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

/// Initiator-rule timeout: when the higher-ID daemon sends and the
/// lower-ID peer is not running, the send should fail with an
/// explanatory error after ~2s.
#[tokio::test]

async fn initiator_rule_timeout_returns_error() {
    let dir_hi = tempdir().unwrap();
    let dir_lo = tempdir().unwrap();

    // Generate identities and ensure we know which is higher.
    let paths_hi = AxonPaths::from_root(PathBuf::from(dir_hi.path()));
    paths_hi.ensure_root_exists().unwrap();
    let id_hi = Identity::load_or_generate(&paths_hi).unwrap();

    let paths_lo = AxonPaths::from_root(PathBuf::from(dir_lo.path()));
    paths_lo.ensure_root_exists().unwrap();
    let id_lo = Identity::load_or_generate(&paths_lo).unwrap();

    // Determine which is higher/lower and set up accordingly.
    let (_hi_id, lo_id, hi_dir, _lo_dir) = if id_hi.agent_id() > id_lo.agent_id() {
        (id_hi, id_lo, dir_hi, dir_lo)
    } else {
        (id_lo, id_hi, dir_lo, dir_hi)
    };

    let port_hi = pick_free_port();
    let port_lo = pick_free_port();

    // Start ONLY the higher-ID daemon with a static peer entry for the lower.
    let peers_for_hi = vec![StaticPeerConfig {
        agent_id: lo_id.agent_id().into(),
        addr: format!("127.0.0.1:{port_lo}").parse().unwrap(),
        pubkey: lo_id.public_key_base64().to_string(),
    }];
    let daemon_hi = spawn_daemon(hi_dir.path(), port_hi, peers_for_hi);
    assert!(wait_for_socket(&daemon_hi.paths, Duration::from_secs(5)).await);

    // The lower-ID peer is NOT running. Send from HI → LO should fail
    // with the initiator-rule error after ~2s.
    let start = tokio::time::Instant::now();
    let reply = ipc_command(
        &daemon_hi.paths.socket,
        json!({
            "cmd": "send",
            "to": lo_id.agent_id(),
            "kind": "ping",
            "payload": {}
        }),
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(reply["ok"], json!(false));
    let error = reply["error"].as_str().unwrap().to_lowercase();
    assert!(
        error.contains("initiator rule"),
        "error should mention initiator rule, got: {error}"
    );
    assert!(
        elapsed >= Duration::from_secs(1),
        "should have waited ~2s, but returned in {:?}",
        elapsed
    );

    // Status counters should not increment.
    let status = ipc_command(&daemon_hi.paths.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    assert_eq!(
        status["messages_sent"].as_u64().unwrap(),
        0,
        "messages_sent should not increment on failed send"
    );

    daemon_hi.shutdown().await;
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
