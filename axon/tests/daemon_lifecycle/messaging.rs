use super::*;

/// Reconnect after peer restart: start two daemons with static peer config,
/// verify they connect, shut one down, then restart it and verify reconnection.
#[tokio::test]

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
        agent_id: id_b.agent_id().into(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: id_b.public_key_base64().to_string(),
    }];
    let peers_for_b = vec![StaticPeerConfig {
        agent_id: id_a.agent_id().into(),
        addr: format!("127.0.0.1:{port_a}").parse().unwrap(),
        pubkey: id_a.public_key_base64().to_string(),
    }];

    // Start both daemons with static peer config.
    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a);
    let (cancel_b, paths_b, handle_b) =
        spawn_daemon(dir_b.path(), port_b, true, peers_for_b.clone());

    assert!(
        wait_for_socket(&paths_a, Duration::from_secs(5)).await,
        "daemon A socket did not appear"
    );
    assert!(
        wait_for_socket(&paths_b, Duration::from_secs(5)).await,
        "daemon B socket did not appear"
    );

    // Wait for initial QUIC connection establishment.
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not connect to B"
    );

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

    // Shut down daemon B and assert it actually stopped.
    cancel_b.cancel();
    timeout(Duration::from_secs(10), handle_b)
        .await
        .expect("daemon B did not stop in time")
        .expect("daemon B task panicked")
        .expect("daemon B exited with error");

    // Wait for A to observe B as disconnected (event-driven, not a fixed sleep).
    assert!(
        wait_for_peer_disconnected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not notice B disconnected"
    );

    // Restart daemon B on the same port.
    let (cancel_b2, paths_b2, handle_b2) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b);
    assert!(
        wait_for_socket(&paths_b2, Duration::from_secs(5)).await,
        "daemon B2 socket did not appear"
    );

    // Wait for reconnection (generous timeout to accommodate exponential backoff).
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(30)).await,
        "daemon A did not reconnect to B"
    );

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

/// End-to-end message delivery: daemon A sends a query to daemon B via IPC,
/// B's IPC client receives the inbound message, and A gets a SendAck + response.
#[tokio::test]

async fn ipc_send_delivers_message_e2e() {
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

    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a);
    let (cancel_b, paths_b, handle_b) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b);

    assert!(wait_for_socket(&paths_a, Duration::from_secs(5)).await);
    assert!(wait_for_socket(&paths_b, Duration::from_secs(5)).await);

    // Wait for daemons to discover and connect to each other.
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not connect to B"
    );

    // Connect an IPC client to B to listen for inbound messages.
    let client_b = UnixStream::connect(&paths_b.socket).await.unwrap();
    let (read_b, _write_b) = client_b.into_split();
    let mut reader_b = BufReader::new(read_b);

    // Send a query from A → B via IPC send command.
    let send_cmd = json!({
        "cmd": "send",
        "to": id_b.agent_id(),
        "kind": "request",
        "payload": {"question": "What is 2+2?", "domain": "math"}
    });
    let ack = ipc_command(&paths_a.socket, send_cmd)
        .await
        .expect("send command failed");
    assert_eq!(ack["ok"], json!(true), "send should return ok=true");
    assert!(
        ack.get("msg_id").is_some(),
        "send should return a msg_id in the ack"
    );

    // B's IPC client should receive the inbound query.
    let mut line = String::new();
    let bytes = timeout(Duration::from_secs(5), reader_b.read_line(&mut line))
        .await
        .expect("timeout waiting for inbound on B")
        .expect("read failed");
    assert!(bytes > 0, "B should receive inbound message");
    let inbound: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(inbound["event"], json!("inbound"));
    assert_eq!(inbound["envelope"]["kind"], "request");
    assert_eq!(inbound["envelope"]["from"], id_a.agent_id());

    cancel_a.cancel();
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
    let _ = timeout(Duration::from_secs(5), handle_b).await;
}

/// Status counters: verify messages_sent and messages_received increment
/// after sending a message between two daemons.
#[tokio::test]

async fn status_counters_increment_after_send() {
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

    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a);
    let (cancel_b, paths_b, handle_b) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b);

    assert!(wait_for_socket(&paths_a, Duration::from_secs(5)).await);
    assert!(wait_for_socket(&paths_b, Duration::from_secs(5)).await);
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not connect to B"
    );

    // Check initial status.
    let status_before = ipc_command(&paths_a.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    let sent_before = status_before["messages_sent"].as_u64().unwrap();

    // Send a ping from A → B.
    let send_cmd = json!({
        "cmd": "send",
        "to": id_b.agent_id(),
        "kind": "request",
        "payload": {}
    });
    let ack = ipc_command(&paths_a.socket, send_cmd).await.unwrap();
    assert_eq!(ack["ok"], json!(true));

    // Brief delay for counters to update.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check status after send.
    let status_after = ipc_command(&paths_a.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    let sent_after = status_after["messages_sent"].as_u64().unwrap();
    assert!(
        sent_after > sent_before,
        "messages_sent should increment after sending (before={sent_before}, after={sent_after})"
    );

    cancel_a.cancel();
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
    let _ = timeout(Duration::from_secs(5), handle_b).await;
}

/// Notify (fire-and-forget) delivered through full daemon stack:
/// A sends a notify via IPC, B's IPC client receives the inbound.
#[tokio::test]

async fn notify_delivered_through_daemon_e2e() {
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

    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a);
    let (cancel_b, paths_b, handle_b) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b);

    assert!(wait_for_socket(&paths_a, Duration::from_secs(5)).await);
    assert!(wait_for_socket(&paths_b, Duration::from_secs(5)).await);
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not connect to B"
    );

    // Connect IPC client to B.
    let client_b = UnixStream::connect(&paths_b.socket).await.unwrap();
    let (read_b, _write_b) = client_b.into_split();
    let mut reader_b = BufReader::new(read_b);

    // Send notify from A → B.
    let send_cmd = json!({
        "cmd": "send",
        "to": id_b.agent_id(),
        "kind": "message",
        "payload": {"topic": "test.event", "data": {"value": 42}, "importance": "high"}
    });
    let ack = ipc_command(&paths_a.socket, send_cmd).await.unwrap();
    assert_eq!(ack["ok"], json!(true));

    // B should receive the notify.
    let mut line = String::new();
    let bytes = timeout(Duration::from_secs(5), reader_b.read_line(&mut line))
        .await
        .expect("timeout waiting for notify on B")
        .expect("read failed");
    assert!(bytes > 0);
    let inbound: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(inbound["event"], json!("inbound"));
    assert_eq!(inbound["envelope"]["kind"], "message");
    assert_eq!(inbound["envelope"]["from"], id_a.agent_id());

    cancel_a.cancel();
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
    let _ = timeout(Duration::from_secs(5), handle_b).await;
}

/// Known peers file is populated with peer info after two daemons connect.
/// Verifies that the periodic save and shutdown save both work.
#[tokio::test]

async fn known_peers_persisted_after_connection() {
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

    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a);
    let (cancel_b, _paths_b, handle_b) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b);

    assert!(wait_for_socket(&paths_a, Duration::from_secs(5)).await);
    assert!(
        wait_for_peer_connected(&paths_a.socket, id_b.agent_id(), Duration::from_secs(10)).await,
        "daemon A did not connect to B"
    );

    // Shut down both daemons gracefully.
    cancel_a.cancel();
    cancel_b.cancel();
    let _ = timeout(Duration::from_secs(10), handle_a).await;
    let _ = timeout(Duration::from_secs(10), handle_b).await;

    // known_peers.json on A should contain B.
    let data = std::fs::read_to_string(&paths_a.known_peers)
        .expect("known_peers.json should exist after shutdown");
    let peers: Vec<Value> = serde_json::from_str(&data).unwrap();
    assert!(
        !peers.is_empty(),
        "known_peers.json should contain at least one peer"
    );
    let has_b = peers
        .iter()
        .any(|p| p["agent_id"].as_str() == Some(id_b.agent_id()));
    assert!(has_b, "known_peers.json should contain daemon B's agent_id");
}
