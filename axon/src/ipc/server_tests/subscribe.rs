use super::*;

#[tokio::test]
async fn v2_client_without_subscribe_gets_no_messages() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default();
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");

    // Send hello (v2 client)
    client
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write");

    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client);
    reader.read_line(&mut line).await.expect("read hello reply");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Broadcast a message
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

    // v2 client without subscribe should NOT receive it
    // Try reading with timeout - should timeout
    let read_result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        reader.read_line(&mut line),
    )
    .await;

    assert!(
        read_result.is_err(),
        "v2 client without subscribe should not receive messages"
    );
}

#[tokio::test]
async fn subscribe_with_replay() {
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

    // Push 2 messages before subscribing
    for i in 1..=2 {
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

    // Get first message ID
    write_half
        .write_all(b"{\"cmd\":\"inbox\",\"limit\":1}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    line.clear();
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    let first_id = v["messages"][0]["envelope"]["id"].as_str().unwrap();

    // Subscribe with since=first_id (should replay second message)
    let subscribe_cmd = format!("{{\"cmd\":\"subscribe\",\"since\":\"{}\"}}\n", first_id);
    write_half
        .write_all(subscribe_cmd.as_bytes())
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");

    // Verify the reply is a subscribe response with replayed count
    match reply {
        DaemonReply::Subscribe {
            ok,
            subscribed,
            replayed,
        } => {
            assert!(ok);
            assert!(subscribed);
            assert!(replayed >= 1, "Expected at least 1 replayed message");
        }
        _ => panic!("Expected Subscribe reply, got {:?}", reply),
    }

    server.send_reply(1, &reply).await.expect("send reply");

    // Read replies - might get subscribe reply and/or replayed messages
    // The order may vary due to async channel behavior
    let mut got_subscribe_reply = false;
    let mut got_replayed_message = false;

    for _ in 0..3 {
        line.clear();
        let read_result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            reader.read_line(&mut line),
        )
        .await;

        if read_result.is_err() {
            break; // Timeout, no more messages
        }

        if line.trim().is_empty() {
            break;
        }

        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();

        if v.get("subscribed").is_some() {
            got_subscribe_reply = true;
            assert_eq!(v["ok"], true);
        } else if v.get("inbound").is_some() {
            got_replayed_message = true;
            assert_eq!(v["inbound"], true);
        }
    }

    assert!(
        got_subscribe_reply || got_replayed_message,
        "Expected to receive subscribe reply or replayed messages"
    );
}

#[tokio::test]
async fn subscription_replacement() {
    // Test: subscribing twice replaces the filter
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

    // Subscribe to "query" only
    write_half
        .write_all(b"{\"cmd\":\"subscribe\",\"kinds\":[\"query\"]}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    line.clear();
    reader.read_line(&mut line).await.expect("read");

    // Now replace subscription with "notify" only
    write_half
        .write_all(b"{\"cmd\":\"subscribe\",\"kinds\":[\"notify\"]}\n")
        .await
        .expect("write");
    let cmd = cmd_rx.recv().await.expect("recv");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");
    line.clear();
    reader.read_line(&mut line).await.expect("read");

    // Send a query message - client should NOT receive it
    let query_envelope = Envelope::new(
        "ed25519.sender".to_string(),
        "ed25519.receiver".to_string(),
        MessageKind::Query,
        json!({"q": "test"}),
    );
    server
        .broadcast_inbound(&query_envelope)
        .await
        .expect("broadcast query");

    // Give time for broadcast
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Send a notify message - client SHOULD receive it
    let notify_envelope = Envelope::new(
        "ed25519.sender".to_string(),
        "ed25519.receiver".to_string(),
        MessageKind::Notify,
        json!({"topic": "test"}),
    );
    server
        .broadcast_inbound(&notify_envelope)
        .await
        .expect("broadcast notify");

    // Read the message - should only get the notify, not the query
    line.clear();
    reader.read_line(&mut line).await.expect("read notify");
    let parsed: serde_json::Value = serde_json::from_str(&line).expect("parse");

    // Should be the notify message
    assert_eq!(parsed["inbound"], true);
    assert_eq!(parsed["envelope"]["kind"], "notify");
}
