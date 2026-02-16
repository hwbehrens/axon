use super::*;

mod inbox_ack;
mod subscribe;

// =========================================================================
// Helpers
// =========================================================================

/// Poll a daemon's inbox via temporary v2 IPC connections until at least
/// `min_count` messages of the given `kind` appear, or panic after 10 s.
pub(crate) async fn wait_for_buffered_messages(
    socket_path: &std::path::Path,
    kind: &str,
    min_count: usize,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for {min_count} buffered {kind} message(s)");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let Ok(stream) = UnixStream::connect(socket_path).await else {
            continue;
        };
        let (read_half, mut writer) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // Hello as a temporary poll consumer
        let hello_cmd =
            "{\"cmd\":\"hello\",\"version\":2,\"consumer\":\"_poll\",\"req_id\":\"ph\"}\n";
        if writer.write_all(hello_cmd.as_bytes()).await.is_err() {
            continue;
        }
        let mut line = String::new();
        if timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .is_err()
        {
            continue;
        }

        // Inbox check
        line.clear();
        let inbox_cmd = "{\"cmd\":\"inbox\",\"limit\":1000,\"req_id\":\"pi\"}\n";
        if writer.write_all(inbox_cmd.as_bytes()).await.is_err() {
            continue;
        }
        if timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .is_err()
        {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
            if let Some(msgs) = v["messages"].as_array() {
                let count = msgs
                    .iter()
                    .filter(|m| m["envelope"]["kind"] == kind)
                    .count();
                if count >= min_count {
                    return;
                }
            }
        }
    }
}

/// Send a v2 hello over a persistent IPC connection. Returns the hello reply.
pub(crate) async fn ipc_hello(
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
pub(crate) async fn ipc_auth(
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
pub(crate) async fn ipc_v2_command(
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
pub(crate) async fn connect_v2(
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
