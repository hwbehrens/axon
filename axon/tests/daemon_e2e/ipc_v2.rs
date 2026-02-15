use super::*;

// =========================================================================
// Helpers
// =========================================================================

/// Send a v2 hello over a persistent IPC connection. Returns the hello reply.
async fn ipc_hello(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    consumer: &str,
    req_id: &str,
) -> Value {
    let cmd = format!(
        "{{\"cmd\":\"hello\",\"version\":2,\"consumer\":\"{consumer}\",\"req_id\":\"{req_id}\"}}\n"
    );
    writer.write_all(cmd.as_bytes()).await.unwrap();
    let mut line = String::new();
    timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("timeout reading hello reply")
        .expect("read hello failed");
    serde_json::from_str(line.trim()).unwrap()
}

/// Send a v2 auth using the token file written by the daemon.
async fn ipc_auth(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    token: &str,
    req_id: &str,
) -> Value {
    let cmd = format!("{{\"cmd\":\"auth\",\"token\":\"{token}\",\"req_id\":\"{req_id}\"}}\n");
    writer.write_all(cmd.as_bytes()).await.unwrap();
    let mut line = String::new();
    timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("timeout reading auth reply")
        .expect("read auth failed");
    serde_json::from_str(line.trim()).unwrap()
}

/// Send a v2 command and read one response, skipping any interleaved inbound events.
async fn ipc_v2_command(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    command: Value,
) -> Value {
    let line = serde_json::to_string(&command).unwrap();
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.write_all(b"\n").await.unwrap();

    // Read lines until we get a response (has "ok" field, not an event)
    loop {
        let mut response = String::new();
        let bytes = timeout(Duration::from_secs(5), reader.read_line(&mut response))
            .await
            .expect("timeout reading v2 response")
            .expect("read failed");
        assert!(bytes > 0, "daemon closed connection");
        let v: Value = serde_json::from_str(response.trim()).unwrap();
        // Skip pushed inbound events (they have "event" field)
        if v.get("event").is_none() {
            return v;
        }
    }
}

/// Connect to daemon, perform v2 hello + peer-cred auth (implicit),
/// returning the split halves. Peer-cred auth happens automatically on
/// the daemon side since tests run as the socket owner.
async fn connect_v2(
    socket_path: &std::path::Path,
    consumer: &str,
) -> (
    tokio::net::unix::OwnedWriteHalf,
    BufReader<tokio::net::unix::OwnedReadHalf>,
) {
    let stream = UnixStream::connect(socket_path).await.unwrap();
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let hello = ipc_hello(&mut write_half, &mut reader, consumer, "h1").await;
    assert_eq!(hello["ok"], json!(true));
    assert_eq!(hello["version"], 2);

    (write_half, reader)
}

// =========================================================================
// Tests
// =========================================================================

/// IPC v2 hello + whoami against a real running daemon.
/// Verifies that the daemon returns its real identity information.
#[tokio::test]
async fn v2_hello_and_whoami_e2e() {
    let td = setup_connected_pair().await;
    let (mut writer, mut reader) = connect_v2(&td.daemon_a.paths.socket, "default").await;

    let whoami = ipc_v2_command(
        &mut writer,
        &mut reader,
        json!({"cmd": "whoami", "req_id": "w1"}),
    )
    .await;

    assert_eq!(whoami["ok"], true);
    assert_eq!(whoami["req_id"], "w1");
    assert_eq!(whoami["ipc_version"], 2);
    // The daemon should return a real agent_id starting with "ed25519."
    assert!(
        whoami["agent_id"].as_str().unwrap().starts_with("ed25519."),
        "agent_id should start with ed25519. prefix"
    );
    assert!(
        whoami["public_key"].as_str().is_some(),
        "should return public_key"
    );
    assert!(
        whoami["uptime_secs"].as_u64().is_some(),
        "should return uptime_secs"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// IPC v2 subscribe receives real messages sent via QUIC between daemons.
/// A sends a notify to B, B's v2-subscribed client receives it as an event.
#[tokio::test]
async fn v2_subscribe_receives_cross_daemon_message() {
    let td = setup_connected_pair().await;

    // Connect a v2 client to B and subscribe
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "default").await;

    let sub_reply = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "subscribe", "replay": false, "req_id": "s1"}),
    )
    .await;
    assert_eq!(sub_reply["ok"], true);
    assert_eq!(sub_reply["subscribed"], true);
    assert_eq!(sub_reply["req_id"], "s1");

    // Send a notify from A → B via v1 IPC (simple)
    let ack = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "notify",
            "payload": {"topic": "v2.test", "data": {"value": 42}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack["ok"], json!(true));

    // B's v2 client should receive the inbound event
    let mut line = String::new();
    let bytes = timeout(Duration::from_secs(5), reader_b.read_line(&mut line))
        .await
        .expect("timeout waiting for v2 inbound event on B")
        .expect("read failed");
    assert!(bytes > 0, "B should receive inbound event");

    let event: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(event["event"], "inbound");
    assert_eq!(event["replay"], false);
    assert!(event["seq"].as_u64().is_some(), "event should have seq");
    assert!(
        event["buffered_at_ms"].as_u64().is_some(),
        "event should have buffered_at_ms"
    );
    assert_eq!(event["envelope"]["kind"], "notify");
    assert_eq!(event["envelope"]["from"], td.id_a.agent_id());

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// IPC v2 inbox/ack round-trip: message is delivered via QUIC, then
/// retrieved via inbox and acknowledged.
#[tokio::test]
async fn v2_inbox_and_ack_with_real_traffic() {
    let td = setup_connected_pair().await;

    // Send a notify from A → B before B's v2 client connects
    let ack = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "notify",
            "payload": {"topic": "buffered.test", "data": {}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack["ok"], json!(true));

    // Wait a bit for the message to arrive at B and be buffered
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Now connect a v2 client to B and fetch inbox
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "default").await;

    let inbox = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "inbox", "limit": 50, "req_id": "i1"}),
    )
    .await;

    assert_eq!(inbox["ok"], true);
    assert_eq!(inbox["req_id"], "i1");
    let messages = inbox["messages"].as_array().unwrap();
    assert!(
        !messages.is_empty(),
        "inbox should contain the buffered message"
    );

    // Find the notify message we sent
    let notify_msg = messages
        .iter()
        .find(|m| m["envelope"]["kind"] == "notify")
        .expect("should find the notify message in inbox");
    assert_eq!(notify_msg["envelope"]["from"], td.id_a.agent_id());
    let seq = notify_msg["seq"].as_u64().unwrap();

    // Ack the message
    let ack_reply = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "ack", "up_to_seq": seq, "req_id": "a1"}),
    )
    .await;
    assert_eq!(ack_reply["ok"], true);
    assert_eq!(ack_reply["acked_seq"], seq);

    // Inbox should now be empty (for this consumer)
    let inbox2 = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "inbox", "limit": 50, "req_id": "i2"}),
    )
    .await;
    assert_eq!(inbox2["ok"], true);

    // Filter to just notify messages to avoid counting hello/etc
    let remaining: Vec<&Value> = inbox2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["envelope"]["kind"] == "notify")
        .collect();
    assert_eq!(
        remaining.len(),
        0,
        "no notify messages should remain after ack"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// IPC v2 token auth against a real daemon: read the token file and authenticate.
#[tokio::test]
async fn v2_token_auth_e2e() {
    let dir = tempdir().unwrap();
    let port = pick_free_port();
    let daemon = spawn_daemon(dir.path(), port, vec![]);
    assert!(wait_for_socket(&daemon.paths, Duration::from_secs(5)).await);

    // The daemon generates a token file at startup
    let token_path = daemon.paths.ipc_token.clone();

    // Wait for token file to appear
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !token_path.exists() {
        if tokio::time::Instant::now() >= deadline {
            panic!("token file did not appear at {}", token_path.display());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let token = std::fs::read_to_string(&token_path)
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(token.len(), 64, "token should be 64 hex chars");

    // Connect, hello, auth with the real token
    let stream = UnixStream::connect(&daemon.paths.socket).await.unwrap();
    let (read_half, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let hello = ipc_hello(&mut writer, &mut reader, "default", "h1").await;
    assert_eq!(hello["ok"], true);
    assert_eq!(hello["version"], 2);

    let auth = ipc_auth(&mut writer, &mut reader, &token, "a1").await;
    assert_eq!(auth["ok"], true);
    assert_eq!(auth["auth"], "accepted");

    // Now v2 commands should work
    let whoami = ipc_v2_command(
        &mut writer,
        &mut reader,
        json!({"cmd": "whoami", "req_id": "w1"}),
    )
    .await;
    assert_eq!(whoami["ok"], true);
    assert_eq!(whoami["ipc_version"], 2);

    daemon.shutdown().await;
}

/// IPC v2 subscribe with replay: messages buffered before subscribe
/// are replayed as events with replay=true.
#[tokio::test]
async fn v2_subscribe_with_replay_e2e() {
    let td = setup_connected_pair().await;

    // Send two messages from A → B before subscribing
    for topic in &["replay.1", "replay.2"] {
        let ack = ipc_command(
            &td.daemon_a.paths.socket,
            json!({
                "cmd": "send",
                "to": td.id_b.agent_id(),
                "kind": "notify",
                "payload": {"topic": topic, "data": {}, "importance": "low"}
            }),
        )
        .await
        .unwrap();
        assert_eq!(ack["ok"], json!(true));
    }

    // Wait for messages to arrive at B
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect v2 client to B and subscribe with replay.
    // We send the subscribe command manually (not via ipc_v2_command) because
    // replayed events are interleaved before the subscribe reply and we need
    // to capture them.
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "default").await;

    let sub_cmd = json!({"cmd": "subscribe", "replay": true, "req_id": "s1"});
    let line_out = serde_json::to_string(&sub_cmd).unwrap();
    writer_b.write_all(line_out.as_bytes()).await.unwrap();
    writer_b.write_all(b"\n").await.unwrap();

    // Read all replay events + the subscribe reply
    let mut notify_replays = Vec::new();
    let mut sub_reply: Option<Value> = None;

    for _ in 0..20 {
        let mut line = String::new();
        let read_result = timeout(Duration::from_millis(2000), reader_b.read_line(&mut line)).await;
        if read_result.is_err() {
            break;
        }
        if line.trim().is_empty() {
            break;
        }
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        if v.get("event").is_some() && v["replay"] == true {
            if v["envelope"]["kind"] == "notify" {
                notify_replays.push(v);
            }
        } else if v.get("subscribed").is_some() {
            sub_reply = Some(v);
            // Subscribe reply comes last; stop reading
            break;
        }
    }

    let sub_reply = sub_reply.expect("should receive subscribe reply");
    assert_eq!(sub_reply["ok"], true);
    assert_eq!(sub_reply["subscribed"], true);
    assert_eq!(sub_reply["req_id"], "s1");
    let replayed = sub_reply["replayed"].as_u64().unwrap();
    assert!(
        replayed >= 2,
        "should replay at least 2 messages, got {replayed}"
    );

    assert_eq!(
        notify_replays.len(),
        2,
        "should replay exactly 2 notify messages"
    );
    assert_eq!(notify_replays[0]["envelope"]["from"], td.id_a.agent_id());
    assert_eq!(notify_replays[1]["envelope"]["from"], td.id_a.agent_id());

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// IPC v2 multi-consumer independence: two consumers have independent
/// cursors over the same daemon's receive buffer.
#[tokio::test]
async fn v2_multi_consumer_e2e() {
    let td = setup_connected_pair().await;

    // Send a message from A → B
    let ack = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "notify",
            "payload": {"topic": "multi.test", "data": {}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack["ok"], json!(true));

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect consumer A
    let (mut writer_a, mut reader_a) = connect_v2(&td.daemon_b.paths.socket, "consumer_a").await;

    // Connect consumer B
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "consumer_b").await;

    // Consumer A: fetch inbox
    let inbox_a = ipc_v2_command(
        &mut writer_a,
        &mut reader_a,
        json!({"cmd": "inbox", "limit": 50, "req_id": "ia1"}),
    )
    .await;
    assert_eq!(inbox_a["ok"], true);
    let msgs_a: Vec<&Value> = inbox_a["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["envelope"]["kind"] == "notify")
        .collect();
    assert!(
        !msgs_a.is_empty(),
        "consumer A should see the notify message"
    );

    // Consumer A: ack the highest seq
    let max_seq = inbox_a["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["seq"].as_u64().unwrap())
        .max()
        .unwrap();
    let ack_a = ipc_v2_command(
        &mut writer_a,
        &mut reader_a,
        json!({"cmd": "ack", "up_to_seq": max_seq, "req_id": "aa1"}),
    )
    .await;
    assert_eq!(ack_a["ok"], true);

    // Consumer B should STILL see messages (independent cursor)
    let inbox_b = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "inbox", "limit": 50, "req_id": "ib1"}),
    )
    .await;
    assert_eq!(inbox_b["ok"], true);
    let msgs_b: Vec<&Value> = inbox_b["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["envelope"]["kind"] == "notify")
        .collect();
    assert!(
        !msgs_b.is_empty(),
        "consumer B should still see the notify (independent cursor)"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// IPC v2 kind-filtered subscribe: only subscribed kinds are delivered.
#[tokio::test]
async fn v2_subscribe_kind_filter_e2e() {
    let td = setup_connected_pair().await;

    // Connect v2 client to B and subscribe to only "notify" kinds
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "default").await;

    let sub_reply = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "subscribe", "replay": false, "kinds": ["notify"], "req_id": "s1"}),
    )
    .await;
    assert_eq!(sub_reply["ok"], true);
    assert_eq!(sub_reply["subscribed"], true);

    // Send a query from A → B (should NOT be delivered to subscriber)
    let ack1 = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "query",
            "payload": {"question": "filtered?", "domain": "test"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack1["ok"], json!(true));

    // Send a notify from A → B (SHOULD be delivered)
    let ack2 = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "notify",
            "payload": {"topic": "filtered.test", "data": {}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack2["ok"], json!(true));

    // Read events from B — should only get the notify, not the query
    let mut received_events = Vec::new();
    for _ in 0..5 {
        let mut line = String::new();
        let read_result = timeout(Duration::from_millis(2000), reader_b.read_line(&mut line)).await;
        if read_result.is_err() {
            break;
        }
        if line.trim().is_empty() {
            break;
        }
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        if v.get("event").is_some() {
            received_events.push(v);
        }
    }

    // We should have received at least the notify
    let notify_events: Vec<&Value> = received_events
        .iter()
        .filter(|e| e["envelope"]["kind"] == "notify")
        .collect();
    let query_events: Vec<&Value> = received_events
        .iter()
        .filter(|e| e["envelope"]["kind"] == "query")
        .collect();

    assert!(!notify_events.is_empty(), "should receive notify events");
    assert!(
        query_events.is_empty(),
        "should NOT receive query events (filtered out)"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}
