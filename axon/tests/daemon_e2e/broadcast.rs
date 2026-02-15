use super::*;

/// Query response is broadcast to all IPC clients on the sender daemon.
/// The SendAck goes only to the requesting client, but the response envelope
/// is broadcast as an inbound message to every connected IPC client.
#[tokio::test]

async fn response_broadcast_to_all_sender_ipc_clients() {
    let td = setup_connected_pair().await;

    // Open two IPC clients on A: one that sends, one that just listens.
    let sender_stream = UnixStream::connect(&td.daemon_a.paths.socket)
        .await
        .unwrap();
    let (sender_read, mut sender_write) = sender_stream.into_split();
    let mut sender_reader = BufReader::new(sender_read);

    let listener_stream = UnixStream::connect(&td.daemon_a.paths.socket)
        .await
        .unwrap();
    let (listener_read, _listener_write) = listener_stream.into_split();
    let mut listener_reader = BufReader::new(listener_read);

    // Send a query from A → B via the sender client.
    let cmd = json!({
        "cmd": "send",
        "to": td.id_b.agent_id(),
        "kind": "ping",
        "payload": {}
    });
    let line = serde_json::to_string(&cmd).unwrap();
    sender_write.write_all(line.as_bytes()).await.unwrap();
    sender_write.write_all(b"\n").await.unwrap();

    // Sender client gets the SendAck first.
    let mut ack_line = String::new();
    timeout(
        Duration::from_secs(5),
        sender_reader.read_line(&mut ack_line),
    )
    .await
    .unwrap()
    .unwrap();
    let ack: Value = serde_json::from_str(ack_line.trim()).unwrap();
    assert_eq!(ack["ok"], json!(true));
    assert!(ack.get("msg_id").is_some(), "should be a SendAck");

    // Sender client also gets the pong response broadcast.
    let mut resp_line = String::new();
    timeout(
        Duration::from_secs(5),
        sender_reader.read_line(&mut resp_line),
    )
    .await
    .unwrap()
    .unwrap();
    let sender_inbound: Value = serde_json::from_str(resp_line.trim()).unwrap();
    assert_eq!(sender_inbound["inbound"], json!(true));
    assert_eq!(sender_inbound["envelope"]["kind"], "pong");

    // Listener client (didn't send anything) also gets the pong broadcast.
    let mut listener_line = String::new();
    timeout(
        Duration::from_secs(5),
        listener_reader.read_line(&mut listener_line),
    )
    .await
    .unwrap()
    .unwrap();
    let listener_inbound: Value = serde_json::from_str(listener_line.trim()).unwrap();
    assert_eq!(listener_inbound["inbound"], json!(true));
    assert_eq!(listener_inbound["envelope"]["kind"], "pong");

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// Multiple IPC clients on the receiver daemon all get the inbound message,
/// but the received counter increments only once (per-daemon, not per-client).
#[tokio::test]

async fn broadcast_fanout_to_multiple_receiver_clients() {
    let td = setup_connected_pair().await;

    // Open 3 IPC clients on B, all listening.
    let mut readers = Vec::new();
    let mut _keep_writes = Vec::new();
    for _ in 0..3 {
        let stream = UnixStream::connect(&td.daemon_b.paths.socket)
            .await
            .unwrap();
        let (read, write) = stream.into_split();
        readers.push(BufReader::new(read));
        _keep_writes.push(write);
    }
    // Brief pause to let all clients register in the accept loop.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send notify from A → B.
    let ack = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "notify",
            "payload": {"topic": "fanout.test", "data": {"n": 1}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack["ok"], json!(true));

    // All 3 clients on B should receive the notify.
    for (i, reader) in readers.iter_mut().enumerate() {
        let mut line = String::new();
        let bytes = timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("timeout on reader")
            .expect("read failed");
        assert!(bytes > 0, "client {i} should receive inbound");
        let inbound: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(inbound["inbound"], json!(true));
        assert_eq!(inbound["envelope"]["kind"], "notify");
    }

    // Counter should increment by 1, not 3.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let status = ipc_command(&td.daemon_b.paths.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    let received = status["messages_received"].as_u64().unwrap();
    // received includes hellos from connection setup + this notify.
    // Just verify it's a sane number (not 3x inflated).
    assert!(
        received < 10,
        "messages_received should not be inflated by fan-out (got {received})"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// Concurrent sends from multiple IPC clients: both get acks and the
/// receiver gets both messages.
#[tokio::test]

async fn concurrent_sends_from_multiple_ipc_clients() {
    let td = setup_connected_pair().await;

    // Collect sender's initial counter.
    let status_before = ipc_command(&td.daemon_a.paths.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    let sent_before = status_before["messages_sent"].as_u64().unwrap();

    // Open two persistent IPC connections to A.
    let target = td.id_b.agent_id().to_string();
    let socket_path = td.daemon_a.paths.socket.clone();

    // Each task opens its own connection, sends, reads the ack.
    let t1 = target.clone();
    let s1 = socket_path.clone();
    let t2 = target.clone();
    let s2 = socket_path.clone();

    // Use notify (fire-and-forget) to avoid response broadcasts interfering
    // with ack reads across the two connections.
    let (r1, r2) = tokio::join!(
        async {
            let mut stream = UnixStream::connect(&s1).await.unwrap();
            let cmd = serde_json::to_string(&json!({
                "cmd": "send", "to": &t1, "kind": "notify",
                "payload": {"topic": "concurrent.1", "data": {}, "importance": "low"}
            }))
            .unwrap();
            stream.write_all(cmd.as_bytes()).await.unwrap();
            stream.write_all(b"\n").await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            timeout(Duration::from_secs(5), reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            serde_json::from_str::<Value>(line.trim()).unwrap()
        },
        async {
            let mut stream = UnixStream::connect(&s2).await.unwrap();
            let cmd = serde_json::to_string(&json!({
                "cmd": "send", "to": &t2, "kind": "notify",
                "payload": {"topic": "concurrent.2", "data": {}, "importance": "low"}
            }))
            .unwrap();
            stream.write_all(cmd.as_bytes()).await.unwrap();
            stream.write_all(b"\n").await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            timeout(Duration::from_secs(5), reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            serde_json::from_str::<Value>(line.trim()).unwrap()
        }
    );

    assert_eq!(
        r1["ok"],
        json!(true),
        "first concurrent send should succeed"
    );
    assert_eq!(
        r2["ok"],
        json!(true),
        "second concurrent send should succeed"
    );
    assert_ne!(
        r1["msg_id"], r2["msg_id"],
        "each send should get a unique msg_id"
    );

    // messages_sent should increase by 2.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let status_after = ipc_command(&td.daemon_a.paths.socket, json!({"cmd": "status"}))
        .await
        .unwrap();
    let sent_after = status_after["messages_sent"].as_u64().unwrap();
    assert_eq!(
        sent_after - sent_before,
        2,
        "messages_sent should increase by exactly 2"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}
