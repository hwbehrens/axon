//! Regression tests: hello(version=1) must enforce v1 semantics (IPC.md §1.2, §7).
//!
//! When a client sends `hello(version=1)` and the daemon negotiates v1,
//! the connection must behave identically to a pre-hello v1 client:
//!   - No req_id requirement
//!   - No auth requirement
//!   - v2-only commands (inbox/ack/subscribe/whoami) rejected as invalid_command
//!   - Legacy broadcast format (not v2 InboundEvent)

use axon::ipc::{CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MessageKind};
use serde_json::json;
use tempfile::tempdir;

/// Create a server with allow_v1=true and token auth configured.
async fn setup_server() -> (IpcServer, tokio::sync::mpsc::Receiver<CommandEvent>) {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        agent_id: "ed25519.testnode".to_string(),
        public_key: "testkey".to_string(),
        allow_v1: true,
        ..IpcServerConfig::default().with_token(Some("a".repeat(64)))
    };
    let path = socket_path.clone();
    std::mem::forget(dir);
    IpcServer::bind(path, 64, config).await.expect("bind")
}

/// Send hello(version=1) and verify it negotiates v1.
async fn hello_v1(server: &IpcServer, client_id: u64) {
    let hello = CommandEvent {
        client_id,
        command: IpcCommand::Hello {
            version: 1,
            req_id: Some("h".to_string()),
            consumer: "default".to_string(),
        },
    };
    let reply = server.handle_command(hello).await.expect("hello");
    match reply {
        DaemonReply::Hello { ok, version, .. } => {
            assert!(ok);
            assert_eq!(version, 1, "must negotiate v1");
        }
        _ => panic!("expected Hello reply, got {:?}", reply),
    }
}

// =========================================================================
// Test 1: No req_id requirement on negotiated v1
// =========================================================================

#[tokio::test]
async fn negotiated_v1_does_not_require_req_id() {
    let (server, _rx) = setup_server().await;
    hello_v1(&server, 1).await;

    // Send status WITHOUT req_id — must succeed (not invalid_command)
    let status = CommandEvent {
        client_id: 1,
        command: IpcCommand::Status { req_id: None },
    };
    let reply = server.handle_command(status).await.expect("status");
    // Should NOT be an error — v1 semantics don't require req_id
    assert!(
        !matches!(
            reply,
            DaemonReply::Error {
                error: IpcErrorCode::InvalidCommand,
                ..
            }
        ),
        "negotiated v1 must not require req_id, got: {:?}",
        reply
    );
}

// =========================================================================
// Test 2: No auth requirement on negotiated v1
// =========================================================================

#[tokio::test]
async fn negotiated_v1_does_not_require_auth() {
    let (server, _rx) = setup_server().await;
    hello_v1(&server, 2).await;

    // Send peers WITHOUT auth — must succeed (not auth_required)
    // Token is configured but v1 semantics bypass auth
    let peers = CommandEvent {
        client_id: 2,
        command: IpcCommand::Peers {
            req_id: Some("p1".to_string()),
        },
    };
    let reply = server.handle_command(peers).await.expect("peers");
    assert!(
        !matches!(
            reply,
            DaemonReply::Error {
                error: IpcErrorCode::AuthRequired,
                ..
            }
        ),
        "negotiated v1 must not require auth, got: {:?}",
        reply
    );
}

// =========================================================================
// Test 3: v2-only commands rejected on negotiated v1
// =========================================================================

#[tokio::test]
async fn negotiated_v1_rejects_v2_only_commands() {
    let (server, _rx) = setup_server().await;
    hello_v1(&server, 3).await;

    // Inbox is v2-only
    let inbox = CommandEvent {
        client_id: 3,
        command: IpcCommand::Inbox {
            limit: 10,
            kinds: None,
            req_id: Some("i1".to_string()),
        },
    };
    let reply = server.handle_command(inbox).await.expect("inbox");
    match reply {
        DaemonReply::Error { error, .. } => {
            assert_eq!(
                error,
                IpcErrorCode::InvalidCommand,
                "v2-only command on negotiated v1 must be invalid_command, not hello_required"
            );
        }
        _ => panic!(
            "expected error for v2-only command on negotiated v1, got: {:?}",
            reply
        ),
    }

    // Subscribe is v2-only
    let subscribe = CommandEvent {
        client_id: 3,
        command: IpcCommand::Subscribe {
            replay: false,
            kinds: None,
            req_id: Some("s1".to_string()),
        },
    };
    let reply = server.handle_command(subscribe).await.expect("subscribe");
    match reply {
        DaemonReply::Error { error, .. } => {
            assert_eq!(error, IpcErrorCode::InvalidCommand);
        }
        _ => panic!(
            "expected error for subscribe on negotiated v1, got: {:?}",
            reply
        ),
    }

    // Whoami is v2-only
    let whoami = CommandEvent {
        client_id: 3,
        command: IpcCommand::Whoami {
            req_id: Some("w1".to_string()),
        },
    };
    let reply = server.handle_command(whoami).await.expect("whoami");
    match reply {
        DaemonReply::Error { error, .. } => {
            assert_eq!(error, IpcErrorCode::InvalidCommand);
        }
        _ => panic!(
            "expected error for whoami on negotiated v1, got: {:?}",
            reply
        ),
    }

    // Ack is v2-only
    let ack = CommandEvent {
        client_id: 3,
        command: IpcCommand::Ack {
            up_to_seq: 1,
            req_id: Some("a1".to_string()),
        },
    };
    let reply = server.handle_command(ack).await.expect("ack");
    match reply {
        DaemonReply::Error { error, .. } => {
            assert_eq!(error, IpcErrorCode::InvalidCommand);
        }
        _ => panic!("expected error for ack on negotiated v1, got: {:?}", reply),
    }
}

// =========================================================================
// Test 4: Negotiated v1 gets legacy broadcast format
// =========================================================================

#[tokio::test]
async fn negotiated_v1_gets_legacy_broadcast() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        allow_v1: true,
        ..Default::default()
    };
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind");

    // Connect a real client and do hello(version=1)
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let client = UnixStream::connect(&socket_path).await.expect("connect");
    let (read_half, mut write_half) = client.into_split();
    let mut reader = BufReader::new(read_half);

    // Wait for client to be registered
    for _ in 0..100 {
        if server.client_count().await >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // Send hello(version=1)
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":1}\n")
        .await
        .expect("write hello");
    let cmd = cmd_rx.recv().await.expect("recv hello");
    let reply = server.handle_command(cmd).await.expect("handle hello");
    server.send_reply(1, &reply).await.expect("send reply");

    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read hello reply");

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

    // Client should receive v1 format (inbound:true), NOT v2 format (event:"inbound")
    line.clear();
    reader.read_line(&mut line).await.expect("read broadcast");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(
        v["inbound"], true,
        "negotiated v1 must get legacy v1 broadcast format"
    );
    assert!(
        v.get("event").is_none(),
        "negotiated v1 must NOT get v2 InboundEvent format"
    );
}
