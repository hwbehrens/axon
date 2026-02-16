use super::*;

/// Graceful shutdown: start a daemon, cancel it, verify IPC socket removed
/// and known_peers.json saved.
#[tokio::test]

async fn graceful_shutdown_cleans_up() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();
    let (cancel, paths, handle) = spawn_daemon(dir.path(), port, true, vec![], None);

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

/// Initiator rule: the daemon with the lower agent_id initiates.
/// Start two daemons and verify the lower-ID one establishes the connection.
#[tokio::test]

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
        agent_id: id_b.agent_id().into(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: id_b.public_key_base64().to_string(),
    }];
    let peers_for_b = vec![StaticPeerConfig {
        agent_id: id_a.agent_id().into(),
        addr: format!("127.0.0.1:{port_a}").parse().unwrap(),
        pubkey: id_a.public_key_base64().to_string(),
    }];

    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a, None);
    let (cancel_b, paths_b, handle_b) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b, None);

    assert!(
        wait_for_socket(&paths_a, Duration::from_secs(5)).await,
        "daemon A socket did not appear"
    );
    assert!(
        wait_for_socket(&paths_b, Duration::from_secs(5)).await,
        "daemon B socket did not appear"
    );

    // Wait for both daemons to see each other as connected.
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not connect to B"
    );
    assert!(
        wait_for_peer_connected(&paths_b.socket, id_a.agent_id(), Duration::from_secs(10)).await,
        "daemon B did not connect to A"
    );

    // Clean up.
    cancel_a.cancel();
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
    let _ = timeout(Duration::from_secs(5), handle_b).await;
}

/// Send to unknown peer: IPC send to an agent_id not in the peer table
/// should return an error with ok=false.
#[tokio::test]

async fn send_to_unknown_peer_returns_error() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();
    let (cancel, paths, handle) = spawn_daemon(dir.path(), port, true, vec![], None);

    assert!(wait_for_socket(&paths, Duration::from_secs(5)).await);

    let send_cmd = json!({
        "cmd": "send",
        "to": "ed25519.deadbeefdeadbeefdeadbeefdeadbeef",
        "kind": "request",
        "payload": {}
    });
    let reply = ipc_command(&paths.socket, send_cmd)
        .await
        .expect("send command should not fail at IPC level");
    assert_eq!(reply["ok"], json!(false), "send to unknown peer must fail");
    assert_eq!(
        reply["error"].as_str().unwrap(),
        "peer_not_found",
        "error should be peer_not_found"
    );

    cancel.cancel();
    let _ = timeout(Duration::from_secs(5), handle).await;
}

/// Peers command reflects connected peer after connection is established.
#[tokio::test]

async fn peers_command_shows_connected_peer() {
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

    let peers_for_a = vec![StaticPeerConfig {
        agent_id: id_b.agent_id().into(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: id_b.public_key_base64().to_string(),
    }];
    let peers_for_b = vec![StaticPeerConfig {
        agent_id: id_a.agent_id().into(),
        addr: format!("127.0.0.1:{port_a}").parse().unwrap(),
        pubkey: id_a.public_key_base64().to_string(),
    }];

    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a, None);
    let (cancel_b, paths_b, handle_b) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b, None);

    assert!(wait_for_socket(&paths_a, Duration::from_secs(5)).await);
    assert!(wait_for_socket(&paths_b, Duration::from_secs(5)).await);
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not connect to B"
    );

    // Check that peers command returns B with source=static.
    let peers_resp = ipc_command(&paths_a.socket, json!({"cmd": "peers"}))
        .await
        .unwrap();
    assert_eq!(peers_resp["ok"], json!(true));
    let peer_list = peers_resp["peers"].as_array().unwrap();
    let b_peer = peer_list
        .iter()
        .find(|p| p["id"].as_str() == Some(id_b.agent_id()))
        .expect("daemon B should appear in A's peer list");
    assert_eq!(b_peer["status"], "connected");
    assert_eq!(b_peer["source"], "static");
    assert!(
        b_peer.get("addr").is_some(),
        "peer should have an addr field"
    );

    cancel_a.cancel();
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
    let _ = timeout(Duration::from_secs(5), handle_b).await;
}
