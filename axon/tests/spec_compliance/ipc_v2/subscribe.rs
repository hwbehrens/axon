//! §3.5 Subscribe replay tests.

use super::*;

// =========================================================================
// §3.5 Subscribe replay
// =========================================================================

/// IPC.md §3.5: subscribe with replay=true replays buffered messages.
/// Uses a real socket connection so replay events are delivered through the
/// client channel and counted in the response.
#[tokio::test]
async fn subscribe_replay_true_replays_buffered() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        token: Some(TOKEN.to_string()),
        ..Default::default()
    };
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .unwrap();

    // Buffer 2 messages before client connects
    for _ in 0..2 {
        server
            .broadcast_inbound(&make_envelope(MessageKind::Query))
            .await
            .unwrap();
    }

    let client = UnixStream::connect(&socket_path).await.unwrap();
    let (read_half, mut write_half) = client.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    // Hello
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2,\"req_id\":\"h1\"}\n")
        .await
        .unwrap();
    let cmd = cmd_rx.recv().await.unwrap();
    let reply = server.handle_command(cmd).await.unwrap();
    server.send_reply(1, &reply).await.unwrap();
    reader.read_line(&mut line).await.unwrap();

    // Auth
    let auth_json = format!("{{\"cmd\":\"auth\",\"token\":\"{TOKEN}\",\"req_id\":\"a1\"}}\n");
    write_half.write_all(auth_json.as_bytes()).await.unwrap();
    let cmd = cmd_rx.recv().await.unwrap();
    let reply = server.handle_command(cmd).await.unwrap();
    server.send_reply(1, &reply).await.unwrap();
    line.clear();
    reader.read_line(&mut line).await.unwrap();

    // Subscribe with replay=true
    write_half
        .write_all(b"{\"cmd\":\"subscribe\",\"replay\":true,\"req_id\":\"s1\"}\n")
        .await
        .unwrap();
    let cmd = cmd_rx.recv().await.unwrap();
    let reply = server.handle_command(cmd).await.unwrap();
    server.send_reply(1, &reply).await.unwrap();

    // Read lines: replay events + subscribe response (order may vary)
    let mut got_subscribe = false;
    let mut replay_count = 0;
    let mut replay_to_seq: Option<u64> = None;

    for _ in 0..4 {
        line.clear();
        let read_result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            reader.read_line(&mut line),
        )
        .await;
        if read_result.is_err() || line.trim().is_empty() {
            break;
        }
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        if v.get("subscribed").is_some() {
            got_subscribe = true;
            assert_eq!(v["ok"], true);
            assert_eq!(v["subscribed"], true);
            assert_eq!(v["req_id"], "s1");
            assert_eq!(v["replayed"], 2);
            replay_to_seq = v["replay_to_seq"].as_u64();
        } else if v.get("event").is_some() {
            assert_eq!(v["event"], "inbound");
            assert_eq!(v["replay"], true);
            assert!(v["seq"].is_u64());
            assert!(v["buffered_at_ms"].is_u64());
            replay_count += 1;
        }
    }

    assert!(got_subscribe, "must receive subscribe response");
    assert_eq!(replay_count, 2, "must replay 2 buffered messages");
    assert!(replay_to_seq.is_some(), "replay_to_seq must be set");
}

/// IPC.md §3.5: subscribe with replay=false skips replay → replayed=0.
#[tokio::test]
async fn subscribe_replay_false_skips_replay() {
    let server = bind_server(test_config()).await;

    // Buffer messages
    server
        .broadcast_inbound(&make_envelope(MessageKind::Query))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Subscribe {
                replay: false,
                kinds: None,
                req_id: Some("s1".into()),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_ok(&reply);
    assert_eq!(json["replayed"], 0);
}
