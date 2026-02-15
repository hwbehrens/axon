//! IPC v2 spec compliance tests.
//!
//! Each test verifies a specific requirement from `spec/IPC.md`.
//! Tests use `IpcServer::handle_command()` directly (bypassing Unix socket I/O)
//! for simplicity and reliability.

use axon::ipc::{CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MessageKind};
use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn test_config() -> IpcServerConfig {
    IpcServerConfig {
        token: Some(TOKEN.to_string()),
        ..Default::default()
    }
}

fn hardened_config() -> IpcServerConfig {
    IpcServerConfig {
        token: Some(TOKEN.to_string()),
        allow_v1: false,
        ..Default::default()
    }
}

async fn bind_server(config: IpcServerConfig) -> IpcServer {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, _rx) = IpcServer::bind(socket_path, 64, config).await.unwrap();
    // Leak tempdir so socket stays alive for the test
    std::mem::forget(dir);
    server
}

/// Send hello v2 + auth for the given client_id, returning the hello reply.
async fn hello_and_auth(server: &IpcServer, client_id: u64) -> DaemonReply {
    hello_and_auth_consumer(server, client_id, "default").await
}

async fn hello_and_auth_consumer(
    server: &IpcServer,
    client_id: u64,
    consumer: &str,
) -> DaemonReply {
    let hello_reply = server
        .handle_command(CommandEvent {
            client_id,
            command: IpcCommand::Hello {
                version: 2,
                req_id: Some("h1".into()),
                consumer: consumer.to_string(),
            },
        })
        .await
        .unwrap();

    server
        .handle_command(CommandEvent {
            client_id,
            command: IpcCommand::Auth {
                token: TOKEN.to_string(),
                req_id: Some("a1".into()),
            },
        })
        .await
        .unwrap();

    hello_reply
}

fn make_envelope(kind: MessageKind) -> Envelope {
    Envelope::new(
        "ed25519.sender1234".to_string(),
        "ed25519.receiver5678".to_string(),
        kind,
        json!({"data": "test"}),
    )
}

fn assert_error(reply: &DaemonReply, expected: IpcErrorCode) {
    let json = serde_json::to_value(reply).unwrap();
    assert_eq!(json["ok"], false, "expected ok=false, got: {json}");
    let error_str = serde_json::to_value(&expected).unwrap();
    assert_eq!(
        json["error"], error_str,
        "expected error={expected}, got: {json}"
    );
}

fn assert_ok(reply: &DaemonReply) {
    let json = serde_json::to_value(reply).unwrap();
    assert_eq!(json["ok"], true, "expected ok=true, got: {json}");
}

// =========================================================================
// §1.2 Hello negotiation
// =========================================================================

/// IPC.md §1.2: client version=2, daemon max=2 → negotiated version=2.
#[tokio::test]
async fn hello_negotiation_v2_client_v2_daemon() {
    let server = bind_server(test_config()).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Hello {
                version: 2,
                req_id: Some("h1".into()),
                consumer: "default".into(),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["version"], 2);
    assert_eq!(json["daemon_max_version"], 2);
    assert_eq!(json["req_id"], "h1");
    assert!(json["agent_id"].is_string());
    assert!(json["features"].is_array());
}

/// IPC.md §1.2: client version=3 capped to daemon max=2.
#[tokio::test]
async fn hello_negotiation_v3_client_capped_to_v2() {
    let server = bind_server(test_config()).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Hello {
                version: 3,
                req_id: Some("h1".into()),
                consumer: "default".into(),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["version"], 2, "negotiated should be min(3, 2) = 2");
    assert_eq!(json["daemon_max_version"], 2);
}

/// IPC.md §1.2: client version=1 → negotiated version=1 (v1 compat).
#[tokio::test]
async fn hello_negotiation_v1_client() {
    let server = bind_server(test_config()).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Hello {
                version: 1,
                req_id: Some("h1".into()),
                consumer: "default".into(),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["version"], 1, "negotiated should be min(1, 2) = 1");
    assert_eq!(json["daemon_max_version"], 2);
}

/// IPC.md §1.2: consumer name is used for buffer operations.
#[tokio::test]
async fn hello_with_consumer() {
    let server = bind_server(test_config()).await;

    // Buffer a message first
    server
        .broadcast_inbound(&make_envelope(MessageKind::Query))
        .await
        .unwrap();

    // Connect with consumer "my-agent"
    hello_and_auth_consumer(&server, 1, "my-agent").await;

    // Inbox should see the buffered message for this consumer
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_ok(&reply);
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
}

// =========================================================================
// §1.3 req_id correlation
// =========================================================================

/// IPC.md §1.3: req_id is echoed on all v2 responses.
#[tokio::test]
async fn req_id_echoed_on_all_v2_responses() {
    let server = bind_server(test_config()).await;

    // Buffer a message so inbox/ack have something to work with
    server
        .broadcast_inbound(&make_envelope(MessageKind::Notify))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    // inbox
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("inbox-1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["req_id"], "inbox-1");

    // Get the seq for ack
    let seq = json["messages"][0]["seq"].as_u64().unwrap();

    // ack
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: seq,
                req_id: Some("ack-1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["req_id"], "ack-1");

    // whoami
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Whoami {
                req_id: Some("whoami-1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["req_id"], "whoami-1");

    // subscribe
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Subscribe {
                replay: false,
                kinds: None,
                req_id: Some("sub-1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["req_id"], "sub-1");
}

/// IPC.md §1.3: v2 command without req_id after hello → invalid_command.
#[tokio::test]
async fn v2_command_without_req_id_rejected() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: None,
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::InvalidCommand);
}

// =========================================================================
// §3.3/§3.4 Inbox/ack cursor semantics
// =========================================================================

/// IPC.md §3.3: inbox messages include seq and buffered_at_ms.
#[tokio::test]
async fn inbox_returns_seq_and_buffered_at_ms() {
    let server = bind_server(test_config()).await;

    server
        .broadcast_inbound(&make_envelope(MessageKind::Query))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_ok(&reply);
    let msg = &json["messages"][0];
    assert!(msg["seq"].is_u64(), "seq must be present and numeric");
    assert!(
        msg["buffered_at_ms"].is_u64(),
        "buffered_at_ms must be present"
    );
    assert!(msg["envelope"].is_object(), "envelope must be present");
}

/// IPC.md §3.4: ack advances cursor; subsequent inbox shows fewer messages.
#[tokio::test]
async fn ack_advances_cursor() {
    let server = bind_server(test_config()).await;

    // Buffer 3 messages
    for _ in 0..3 {
        server
            .broadcast_inbound(&make_envelope(MessageKind::Query))
            .await
            .unwrap();
    }

    hello_and_auth(&server, 1).await;

    // Fetch all
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    let msgs = json["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);

    // Ack first two (seq of second message)
    let seq_2 = msgs[1]["seq"].as_u64().unwrap();
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: seq_2,
                req_id: Some("r2".into()),
            },
        })
        .await
        .unwrap();
    assert_ok(&reply);

    // Inbox should now show only 1 message
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r3".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
}

/// IPC.md §3.4: ack beyond delivered → ack_out_of_range.
#[tokio::test]
async fn ack_out_of_range_rejected() {
    let server = bind_server(test_config()).await;

    server
        .broadcast_inbound(&make_envelope(MessageKind::Query))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    // Fetch inbox to establish delivered seq
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    let max_seq = json["next_seq"].as_u64().unwrap();

    // Ack beyond delivered
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: max_seq + 100,
                req_id: Some("r2".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::AckOutOfRange);
}

/// IPC.md §4.3: two consumers ack independently.
#[tokio::test]
async fn multi_consumer_cursor_independence() {
    let server = bind_server(test_config()).await;

    // Buffer 3 messages
    for _ in 0..3 {
        server
            .broadcast_inbound(&make_envelope(MessageKind::Notify))
            .await
            .unwrap();
    }

    // Consumer A (client 1)
    hello_and_auth_consumer(&server, 1, "consumer-a").await;
    // Consumer B (client 2)
    hello_and_auth_consumer(&server, 2, "consumer-b").await;

    // Consumer A: fetch inbox, ack all
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    let max_seq = json["next_seq"].as_u64().unwrap();
    server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: max_seq,
                req_id: Some("r2".into()),
            },
        })
        .await
        .unwrap();

    // Consumer A inbox should now be empty
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r3".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["messages"].as_array().unwrap().len(), 0);

    // Consumer B should still see all 3
    let reply = server
        .handle_command(CommandEvent {
            client_id: 2,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r4".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["messages"].as_array().unwrap().len(), 3);
}

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

// =========================================================================
// §5 Error codes
// =========================================================================

/// IPC.md §5: v2 command without hello → hello_required.
#[tokio::test]
async fn error_code_hello_required() {
    let server = bind_server(test_config()).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::HelloRequired);
}

/// IPC.md §5: after hello, v2 command without auth → auth_required.
#[tokio::test]
async fn error_code_auth_required() {
    let server = bind_server(test_config()).await;

    // Hello only, no auth
    server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Hello {
                version: 2,
                req_id: Some("h1".into()),
                consumer: "default".into(),
            },
        })
        .await
        .unwrap();

    // Attempt v2 command without auth
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Whoami {
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::AuthRequired);
}

/// IPC.md §5: inbox with unknown kind → invalid_command.
#[tokio::test]
async fn error_code_invalid_command_unknown_kind() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: Some(vec!["bogus".to_string()]),
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::InvalidCommand);
}

// =========================================================================
// §2.4 Hardened mode
// =========================================================================

/// IPC.md §2.4: hardened mode rejects v1 commands before hello.
#[tokio::test]
async fn hardened_mode_rejects_v1_commands() {
    let server = bind_server(hardened_config()).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Status { req_id: None },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::HelloRequired);
}

/// IPC.md §2.4: hardened mode rejects hello negotiating v1.
#[tokio::test]
async fn hardened_mode_rejects_v1_negotiation() {
    let server = bind_server(hardened_config()).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Hello {
                version: 1,
                req_id: Some("h1".into()),
                consumer: "default".into(),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::UnsupportedVersion);
}

// =========================================================================
// §3.3 Unknown kind rejection
// =========================================================================

/// IPC.md §3.3: inbox with unknown kind → invalid_command.
#[tokio::test]
async fn inbox_unknown_kind_rejected() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: Some(vec!["bogus".to_string()]),
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::InvalidCommand);
}

/// IPC.md §3.5: subscribe with unknown kind → invalid_command.
#[tokio::test]
async fn subscribe_unknown_kind_rejected() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Subscribe {
                replay: false,
                kinds: Some(vec!["bogus".to_string()]),
                req_id: Some("s1".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::InvalidCommand);
}
