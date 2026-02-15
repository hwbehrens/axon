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

    // v2 commands require hello
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write hello");
    let cmd = cmd_rx.recv().await.expect("recv hello");
    let reply = server.handle_command(cmd).await.expect("handle hello");
    server
        .send_reply(1, &reply)
        .await
        .expect("send hello reply");
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read hello reply");

    // Fetch inbox
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"r1\"}\n")
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

    // Extract highest seq from messages
    let max_seq = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["seq"].as_u64().unwrap())
        .max()
        .unwrap();

    // Ack messages using seq-based cursor
    let ack_cmd = format!(
        "{{\"cmd\":\"ack\",\"up_to_seq\":{},\"req_id\":\"r2\"}}\n",
        max_seq
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
    assert_eq!(v["acked_seq"], max_seq);

    // Verify buffer is now empty
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"r3\"}\n")
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
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"r1\"}\n")
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
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"r1\"}\n")
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
async fn inbox_seq_cursor_semantics() {
    // Verifies seq-based cursor: push messages, fetch, ack some, fetch again gets remaining
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

    // Fetch all messages
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"r1\"}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 3);

    // Ack only the first message (seq 1)
    let first_seq = v["messages"][0]["seq"].as_u64().unwrap();
    let ack_cmd = format!(
        "{{\"cmd\":\"ack\",\"up_to_seq\":{},\"req_id\":\"r2\"}}\n",
        first_seq
    );
    write_half
        .write_all(ack_cmd.as_bytes())
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    line.clear();
    reader.read_line(&mut line).await.expect("read");

    // Fetch again — should get remaining 2 messages
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"r3\"}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["messages"].as_array().unwrap().len(), 2);
    assert_eq!(v["messages"][0]["envelope"]["to"], "ed25519.receiver2");
    assert_eq!(v["messages"][1]["envelope"]["to"], "ed25519.receiver3");
}

#[tokio::test]
async fn multi_consumer_inbox_independence() {
    // Test: two consumers with different names have independent ack cursors
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default();
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Push 3 messages before clients connect
    for i in 1..=3 {
        let envelope = Envelope::new(
            "ed25519.sender".to_string(),
            format!("ed25519.receiver{}", i),
            MessageKind::Notify,
            json!({"msg": i}),
        );
        server
            .broadcast_inbound(&envelope)
            .await
            .expect("broadcast");
    }

    // Connect consumer A
    let client_a = UnixStream::connect(&socket_path).await.expect("connect A");
    let (read_a, mut write_a) = client_a.into_split();
    let mut reader_a = BufReader::new(read_a);

    write_a
        .write_all(b"{\"cmd\":\"hello\",\"version\":2,\"consumer\":\"consumer_a\"}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    let mut line_a = String::new();
    reader_a.read_line(&mut line_a).await.expect("read");

    // Connect consumer B
    let client_b = UnixStream::connect(&socket_path).await.expect("connect B");
    let (read_b, mut write_b) = client_b.into_split();
    let mut reader_b = BufReader::new(read_b);

    write_b
        .write_all(b"{\"cmd\":\"hello\",\"version\":2,\"consumer\":\"consumer_b\"}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(2, &reply).await.expect("send reply");
    let mut line_b = String::new();
    reader_b.read_line(&mut line_b).await.expect("read");

    // Consumer A: fetch inbox (should see all 3)
    write_a
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"a1\"}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    line_a.clear();
    reader_a.read_line(&mut line_a).await.expect("read");
    let va: serde_json::Value = serde_json::from_str(line_a.trim()).unwrap();
    assert_eq!(va["messages"].as_array().unwrap().len(), 3);

    // Consumer A: ack all 3
    let max_seq = va["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["seq"].as_u64().unwrap())
        .max()
        .unwrap();
    let ack_cmd = format!(
        "{{\"cmd\":\"ack\",\"up_to_seq\":{},\"req_id\":\"a2\"}}\n",
        max_seq
    );
    write_a.write_all(ack_cmd.as_bytes()).await.expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    line_a.clear();
    reader_a.read_line(&mut line_a).await.expect("read");

    // Consumer B: fetch inbox (should still see all 3 — independent cursor)
    write_b
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":10,\"req_id\":\"b1\"}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(2, &reply).await.expect("send reply");
    line_b.clear();
    reader_b.read_line(&mut line_b).await.expect("read");
    let vb: serde_json::Value = serde_json::from_str(line_b.trim()).unwrap();
    assert_eq!(vb["ok"], true);
    assert_eq!(
        vb["messages"].as_array().unwrap().len(),
        3,
        "consumer B should still see all 3 messages (independent cursor)"
    );
}
