use super::*;

#[tokio::test]
async fn inbox_and_ack_round_trip() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default();
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Add some messages to the buffer
    for i in 0..3 {
        let envelope = Envelope::new(
            "ed25519.sender".to_string(),
            "ed25519.receiver".to_string(),
            MessageKind::Query,
            json!({"question": format!("test {}", i)}),
        );
        server
            .broadcast_inbound(&envelope)
            .await
            .expect("broadcast");
    }

    let client = UnixStream::connect(&socket_path).await.expect("connect");
    let (read_half, mut write_half) = client.into_split();
    let mut reader = BufReader::new(read_half);

    // Fetch inbox
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10}\n")
        .await
        .expect("write");

    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read");

    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["messages"].as_array().unwrap().len(), 3);

    // Extract message IDs
    let ids: Vec<String> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["envelope"]["id"].as_str().unwrap().to_string())
        .collect();

    // Ack messages
    let ack_cmd = format!(
        "{{\"cmd\":\"ack\",\"ids\":{}}}\n",
        serde_json::to_string(&ids).unwrap()
    );
    write_half
        .write_all(ack_cmd.as_bytes())
        .await
        .expect("write");

    let cmd = cmd_rx.recv().await.expect("recv ack");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");

    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["acked"], 3);

    // Verify buffer is now empty
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10}\n")
        .await
        .expect("write");

    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");

    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn buffer_ttl_eviction() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    // Set very short TTL (1 second)
    let config = IpcServerConfig {
        buffer_ttl_secs: 1,
        ..Default::default()
    };
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");
    let (mut read_half, mut write_half) = client.split();
    let mut reader = BufReader::new(&mut read_half);

    // Authenticate client
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read");

    // Push a message
    let envelope = Envelope::new(
        "ed25519.sender".to_string(),
        "ed25519.receiver".to_string(),
        MessageKind::Notify,
        json!({"topic": "test"}),
    );
    server
        .broadcast_inbound(&envelope)
        .await
        .expect("broadcast");

    // Wait for TTL to expire (1.5 seconds to be safe)
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    // Fetch - should be empty due to TTL eviction
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["messages"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn buffer_capacity_eviction() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    // Set capacity to 3
    let config = IpcServerConfig {
        buffer_size: 3,
        ..Default::default()
    };
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");
    let (mut read_half, mut write_half) = client.split();
    let mut reader = BufReader::new(&mut read_half);

    // Authenticate client
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read");

    // Push 4 messages
    for i in 1..=4 {
        let envelope = Envelope::new(
            "ed25519.sender".to_string(),
            format!("ed25519.receiver{}", i),
            MessageKind::Notify,
            json!({"topic": format!("test{}", i)}),
        );
        server
            .broadcast_inbound(&envelope)
            .await
            .expect("broadcast");
    }

    // Fetch - should have only 3 messages (oldest dropped)
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["messages"].as_array().unwrap().len(), 3);

    // Verify it's messages 2, 3, 4 (message 1 was evicted)
    let messages = v["messages"].as_array().unwrap();
    assert_eq!(messages[0]["envelope"]["to"], "ed25519.receiver2");
    assert_eq!(messages[1]["envelope"]["to"], "ed25519.receiver3");
    assert_eq!(messages[2]["envelope"]["to"], "ed25519.receiver4");
}

#[tokio::test]
async fn inbox_since_parameter_uuid() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default();
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");
    let (mut read_half, mut write_half) = client.split();
    let mut reader = BufReader::new(&mut read_half);

    // Authenticate client
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read");

    // Push 3 messages
    for i in 1..=3 {
        let envelope = Envelope::new(
            "ed25519.sender".to_string(),
            format!("ed25519.receiver{}", i),
            MessageKind::Notify,
            json!({"topic": format!("test{}", i)}),
        );
        server
            .broadcast_inbound(&envelope)
            .await
            .expect("broadcast");
    }

    // Fetch all to get IDs
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();

    // Get second message UUID
    let second_id = v["messages"][1]["envelope"]["id"].as_str().unwrap();

    // Fetch with since=second_id, should get only third message
    let inbox_cmd = format!(
        "{{\"cmd\":\"inbox\",\"limit\":10,\"since\":\"{}\"}}\n",
        second_id
    );
    write_half
        .write_all(inbox_cmd.as_bytes())
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    assert_eq!(v["messages"][0]["envelope"]["to"], "ed25519.receiver3");
}

#[tokio::test]
async fn inbox_since_iso_timestamp() {
    // Test: inbox with since parameter as ISO timestamp
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default();
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");
    let (mut read_half, mut write_half) = client.split();
    let mut reader = BufReader::new(&mut read_half);

    // Authenticate
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read");

    // Push message 1
    let envelope1 = Envelope::new(
        "ed25519.sender1".to_string(),
        "ed25519.receiver".to_string(),
        MessageKind::Notify,
        json!({"msg": 1}),
    );
    server
        .broadcast_inbound(&envelope1)
        .await
        .expect("broadcast 1");

    // Small delay to ensure different timestamps
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Capture timestamp between message 2 and 3
    let middle_timestamp = chrono::Utc::now().to_rfc3339();

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Push message 2
    let envelope2 = Envelope::new(
        "ed25519.sender2".to_string(),
        "ed25519.receiver".to_string(),
        MessageKind::Notify,
        json!({"msg": 2}),
    );
    server
        .broadcast_inbound(&envelope2)
        .await
        .expect("broadcast 2");

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Push message 3
    let envelope3 = Envelope::new(
        "ed25519.sender3".to_string(),
        "ed25519.receiver".to_string(),
        MessageKind::Notify,
        json!({"msg": 3}),
    );
    server
        .broadcast_inbound(&envelope3)
        .await
        .expect("broadcast 3");

    // Request inbox with since = middle_timestamp
    let inbox_cmd = format!(
        "{{\"cmd\":\"inbox\",\"limit\":10,\"since\":\"{}\"}}\n",
        middle_timestamp
    );
    write_half
        .write_all(inbox_cmd.as_bytes())
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");

    match reply {
        DaemonReply::Inbox { messages, .. } => {
            // Should get messages 2 and 3 (after the timestamp)
            assert_eq!(
                messages.len(),
                2,
                "should only get messages after timestamp"
            );
            // Verify it's message 2 and 3
            let payload1: serde_json::Value =
                serde_json::from_str(messages[0].envelope.payload.get()).expect("parse payload1");
            let payload2: serde_json::Value =
                serde_json::from_str(messages[1].envelope.payload.get()).expect("parse payload2");
            assert_eq!(payload1["msg"], 2);
            assert_eq!(payload2["msg"], 3);
        }
        _ => panic!("Expected Inbox reply, got {:?}", reply),
    }
}
