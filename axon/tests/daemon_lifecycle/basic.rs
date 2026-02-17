use super::*;

/// Graceful shutdown: start a daemon, cancel it, verify IPC socket removed
/// and known_peers.json saved.
#[tokio::test]

async fn graceful_shutdown_cleans_up() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();
    let (cancel, paths, handle) = spawn_daemon(dir.path(), port, true, vec![]);
    let pid_path = paths.root.join("daemon.pid");

    // Wait for daemon to be ready.
    assert!(
        wait_for_socket(&paths, Duration::from_secs(5)).await,
        "daemon socket did not appear"
    );
    assert!(
        pid_path.exists(),
        "daemon.pid should exist while daemon runs"
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
    assert!(
        !pid_path.exists(),
        "daemon.pid was not removed after shutdown"
    );

    // known_peers.json should exist (saved during shutdown).
    assert!(
        paths.known_peers.exists(),
        "known_peers.json was not saved during shutdown"
    );
}

/// A second daemon on the same state root should fail fast with a clear error.
#[tokio::test]
async fn second_daemon_same_state_root_is_rejected() {
    let dir = tempdir().unwrap();
    let port_a = pick_free_port();
    let port_b = pick_free_port();

    let (cancel_a, paths, handle_a) = spawn_daemon(dir.path(), port_a, true, vec![]);
    assert!(
        wait_for_socket(&paths, Duration::from_secs(5)).await,
        "first daemon socket did not appear"
    );

    let second_cancel = CancellationToken::new();
    let second_opts = DaemonOptions {
        port: Some(port_b),
        disable_mdns: true,
        axon_root: Some(PathBuf::from(dir.path())),
        cancel: Some(second_cancel.clone()),
    };
    let second_handle = tokio::spawn(async move { run_daemon(second_opts).await });

    let second_result = timeout(Duration::from_secs(3), second_handle)
        .await
        .expect("second daemon should fail immediately")
        .expect("second daemon task panicked");
    let err = second_result.expect_err("second daemon should be rejected by lock file");
    assert!(
        err.to_string().contains("daemon already running (pid"),
        "unexpected error: {err:#}"
    );

    second_cancel.cancel();
    cancel_a.cancel();
    let _ = timeout(Duration::from_secs(5), handle_a).await;
}

/// SIGTERM should trigger graceful shutdown and cleanup via the signal handler.
#[test]
fn sigterm_shutdown_cleans_socket_and_lock_file() {
    let bin = axon_bin();
    let dir = tempdir().unwrap();
    let port = pick_free_port();
    let root_str = dir.path().to_str().expect("utf8 path");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let pid_path = paths.root.join("daemon.pid");

    let mut child = std::process::Command::new(&bin)
        .args([
            "--state-root",
            root_str,
            "daemon",
            "--disable-mdns",
            "--port",
            &port.to_string(),
        ])
        .spawn()
        .expect("failed to spawn daemon");

    let ready_deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < ready_deadline {
        if paths.socket.exists() && pid_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(paths.socket.exists(), "daemon socket did not appear");
    assert!(pid_path.exists(), "daemon.pid did not appear");

    // SAFETY: kill sends SIGTERM to the child process ID created by this test.
    let rc = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0, "failed to send SIGTERM to daemon process");

    let shutdown_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("failed waiting for daemon process") {
            break status;
        }
        if std::time::Instant::now() >= shutdown_deadline {
            let _ = child.kill();
            panic!("daemon did not exit after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "daemon exited with status: {status}");

    assert!(
        !paths.socket.exists(),
        "IPC socket should be removed after SIGTERM shutdown"
    );
    assert!(
        !pid_path.exists(),
        "daemon.pid should be removed after SIGTERM shutdown"
    );
}

/// Both daemons connect: either side can dial. Start two daemons and
/// verify both see each other as connected.
#[tokio::test]

async fn both_sides_connect() {
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
    let (cancel, paths, handle) = spawn_daemon(dir.path(), port, true, vec![]);

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

/// Send to own agent ID should return self_send instead of peer_not_found.
#[tokio::test]
async fn send_to_self_returns_specific_error() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();
    let (cancel, paths, handle) = spawn_daemon(dir.path(), port, true, vec![]);

    assert!(wait_for_socket(&paths, Duration::from_secs(5)).await);

    let local_identity = Identity::load_or_generate(&paths).expect("load local identity");
    let send_cmd = json!({
        "cmd": "send",
        "to": local_identity.agent_id(),
        "kind": "request",
        "payload": {}
    });
    let reply = ipc_command(&paths.socket, send_cmd)
        .await
        .expect("send command should not fail at IPC level");
    assert_eq!(reply["ok"], json!(false), "send to self must fail");
    assert_eq!(
        reply["error"].as_str().unwrap(),
        "self_send",
        "error should be self_send"
    );
    assert_eq!(
        reply["message"].as_str().unwrap(),
        "cannot send messages to self",
        "self_send message should be actionable"
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

    let (cancel_a, paths_a, handle_a) = spawn_daemon(dir_a.path(), port_a, true, peers_for_a);
    let (cancel_b, paths_b, handle_b) = spawn_daemon(dir_b.path(), port_b, true, peers_for_b);

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
        .find(|p| p["agent_id"].as_str() == Some(id_b.agent_id()))
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
