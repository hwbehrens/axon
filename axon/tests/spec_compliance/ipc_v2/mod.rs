//! IPC v2 spec compliance tests.
//!
//! Each test verifies a specific requirement from `spec/IPC.md`.
//! Tests use `IpcServer::handle_command()` directly (bypassing Unix socket I/O)
//! for simplicity and reliability.

use axon::ipc::{CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MessageKind};
use serde_json::json;

mod error_codes;
mod inbox_ack;
mod subscribe;

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
