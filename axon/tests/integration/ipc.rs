use crate::*;

// =========================================================================
// IPC integration
// =========================================================================

/// IPC server accepts connection and routes commands.
#[tokio::test]
async fn ipc_send_command_roundtrip() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
        .await
        .unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    // Send a "peers" command.
    client.write_all(b"{\"cmd\":\"peers\"}\n").await.unwrap();

    let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(cmd.command, IpcCommand::Peers));

    // Reply.
    server
        .send_reply(
            cmd.client_id,
            &DaemonReply::Peers {
                ok: true,
                peers: vec![],
            },
        )
        .await
        .unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client);
    reader.read_line(&mut line).await.unwrap();
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["peers"], json!([]));
}

/// IPC status command returns expected fields.
#[tokio::test]
async fn ipc_status_roundtrip() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
        .await
        .unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    client.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();

    let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(cmd.command, IpcCommand::Status));

    server
        .send_reply(
            cmd.client_id,
            &DaemonReply::Status {
                ok: true,
                uptime_secs: 99,
                peers_connected: 2,
                messages_sent: 10,
                messages_received: 5,
            },
        )
        .await
        .unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client);
    reader.read_line(&mut line).await.unwrap();
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["uptime_secs"], 99);
    assert_eq!(v["peers_connected"], 2);
    assert_eq!(v["messages_sent"], 10);
    assert_eq!(v["messages_received"], 5);
}

/// Multiple sequential IPC commands on the same connection.
#[tokio::test]
async fn ipc_multiple_commands_sequential() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
        .await
        .unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Send three commands in sequence.
    for i in 0..3 {
        client.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();

        let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .unwrap()
            .unwrap();

        server
            .send_reply(
                cmd.client_id,
                &DaemonReply::Status {
                    ok: true,
                    uptime_secs: i + 1,
                    peers_connected: 0,
                    messages_sent: 0,
                    messages_received: 0,
                },
            )
            .await
            .unwrap();

        let mut line = String::new();
        let mut reader = BufReader::new(&mut client);
        reader.read_line(&mut line).await.unwrap();
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["uptime_secs"], i + 1);
    }
}

/// Invalid IPC command returns error without crashing.
#[tokio::test]
async fn ipc_invalid_command_returns_error() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (_server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
        .await
        .unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    client
        .write_all(b"{\"cmd\":\"nonexistent\"}\n")
        .await
        .unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(client);
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"ok\":false"));
    assert!(line.contains("invalid command"));
}

/// IPC send command with ref field deserializes correctly.
#[test]
fn ipc_send_with_ref_deserializes() {
    let input = r#"{"cmd":"send","to":"ed25519.deadbeef01234567deadbeef01234567","kind":"cancel","payload":{"reason":"changed plans"},"ref":"550e8400-e29b-41d4-a716-446655440000"}"#;
    let cmd: IpcCommand = serde_json::from_str(input).unwrap();
    match cmd {
        IpcCommand::Send {
            to, kind, ref_id, ..
        } => {
            assert_eq!(to, "ed25519.deadbeef01234567deadbeef01234567");
            assert_eq!(kind, MessageKind::Cancel);
            assert!(ref_id.is_some());
        }
        _ => panic!("expected Send"),
    }
}

/// IPC send without ref defaults to None.
#[test]
fn ipc_send_without_ref_defaults_to_none() {
    let input = r#"{"cmd":"send","to":"ed25519.deadbeef01234567deadbeef01234567","kind":"query","payload":{"question":"hello?"}}"#;
    let cmd: IpcCommand = serde_json::from_str(input).unwrap();
    match cmd {
        IpcCommand::Send { ref_id, .. } => {
            assert!(ref_id.is_none());
        }
        _ => panic!("expected Send"),
    }
}

/// IPC client disconnect does not affect other clients.
#[tokio::test]
async fn ipc_client_disconnect_isolation() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
        .await
        .unwrap();

    let client_a = UnixStream::connect(&socket_path).await.unwrap();
    let mut client_b = UnixStream::connect(&socket_path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.client_count().await, 2);

    // Drop client A.
    drop(client_a);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Client B should still receive broadcasts.
    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Notify,
        json!({"topic": "meta.status", "data": {}}),
    );
    server.broadcast_inbound(&envelope).await.unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client_b);
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"inbound\":true"));
}

/// IPC broadcasts to multiple simultaneous clients.
#[tokio::test]
async fn ipc_broadcast_to_all_clients() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
        .await
        .unwrap();

    let mut client_a = UnixStream::connect(&socket_path).await.unwrap();
    let mut client_b = UnixStream::connect(&socket_path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Ping,
        json!({}),
    );
    server.broadcast_inbound(&envelope).await.unwrap();

    let mut line_a = String::new();
    let mut line_b = String::new();
    let mut reader_a = BufReader::new(&mut client_a);
    let mut reader_b = BufReader::new(&mut client_b);
    reader_a.read_line(&mut line_a).await.unwrap();
    reader_b.read_line(&mut line_b).await.unwrap();
    assert!(line_a.contains("\"inbound\":true"));
    assert!(line_b.contains("\"inbound\":true"));
}

/// IPC cleanup removes socket file.
#[tokio::test]
async fn ipc_cleanup_removes_socket() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
        .await
        .unwrap();

    assert!(socket_path.exists());
    server.cleanup_socket().unwrap();
    assert!(!socket_path.exists());
}
