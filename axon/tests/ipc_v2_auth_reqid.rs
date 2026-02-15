//! Integration tests for IPC v2 auth/req_id enforcement (remediation plan Phase 0).
//!
//! These tests verify that the fixes for Critical issues #1–#2 and
//! Major/Minor issues #3, #5, #7, #8 are in place. They exercise
//! `IpcServer::handle_command` directly (no socket I/O) to avoid
//! peer-credential auto-auth interfering with auth gating tests.

use axon::ipc::{CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcServer, IpcServerConfig};
use axon::message::MessageKind;
use serde_json::json;
use tempfile::tempdir;

// =========================================================================
// Helpers
// =========================================================================

/// Create a server with token auth required (no peer-cred auto-auth).
async fn setup_server_with_token(
    token: &str,
) -> (IpcServer, tokio::sync::mpsc::Receiver<CommandEvent>) {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        agent_id: "ed25519.testnode".to_string(),
        public_key: "testkey".to_string(),
        token: Some(token.to_string()),
        ..Default::default()
    };
    let path = socket_path.clone();
    std::mem::forget(dir);
    IpcServer::bind(path, 64, config).await.expect("bind")
}

/// Hello + auth a synthetic client (bypasses peer creds).
async fn hello_and_auth(server: &IpcServer, client_id: u64, token: &str) {
    let hello = CommandEvent {
        client_id,
        command: IpcCommand::Hello {
            version: 2,
            req_id: Some("h".to_string()),
            consumer: "default".to_string(),
        },
    };
    let reply = server.handle_command(hello).await.expect("hello");
    assert!(matches!(reply, DaemonReply::Hello { ok: true, .. }));

    let auth = CommandEvent {
        client_id,
        command: IpcCommand::Auth {
            token: token.to_string(),
            req_id: Some("a".to_string()),
        },
    };
    let reply = server.handle_command(auth).await.expect("auth");
    assert!(matches!(reply, DaemonReply::Auth { ok: true, .. }));
}

// =========================================================================
// Test 1: Auth bypass (Issue #1)
// =========================================================================

/// v2 client does hello then sends `send` without `auth` → must get `auth_required`.
#[tokio::test]
async fn v2_send_without_auth_returns_auth_required() {
    let token = "a".repeat(64);
    let (server, _rx) = setup_server_with_token(&token).await;

    let hello = CommandEvent {
        client_id: 100,
        command: IpcCommand::Hello {
            version: 2,
            req_id: Some("h1".to_string()),
            consumer: "default".to_string(),
        },
    };
    server.handle_command(hello).await.unwrap();

    let send = CommandEvent {
        client_id: 100,
        command: IpcCommand::Send {
            to: "ed25519.target".to_string(),
            kind: MessageKind::Notify,
            payload: json!({}),
            ref_id: None,
            req_id: Some("s1".to_string()),
        },
    };
    let reply = server.handle_command(send).await.unwrap();
    match reply {
        DaemonReply::Error { error, .. } => assert_eq!(error, IpcErrorCode::AuthRequired),
        _ => panic!("expected auth_required, got {:?}", reply),
    }
}

// =========================================================================
// Test 2: req_id enforcement (Issue #2)
// =========================================================================

/// v2 client sends `peers`/`status`/`send` without `req_id` → must get `invalid_command`.
#[tokio::test]
async fn v2_command_without_req_id_returns_invalid_command() {
    let token = "a".repeat(64);
    let (server, _rx) = setup_server_with_token(&token).await;
    hello_and_auth(&server, 200, &token).await;

    // Peers without req_id
    let peers = CommandEvent {
        client_id: 200,
        command: IpcCommand::Peers { req_id: None },
    };
    let reply = server.handle_command(peers).await.unwrap();
    match reply {
        DaemonReply::Error { error, .. } => assert_eq!(error, IpcErrorCode::InvalidCommand),
        _ => panic!("expected invalid_command, got {:?}", reply),
    }

    // Status without req_id
    let status = CommandEvent {
        client_id: 200,
        command: IpcCommand::Status { req_id: None },
    };
    let reply = server.handle_command(status).await.unwrap();
    match reply {
        DaemonReply::Error { error, .. } => assert_eq!(error, IpcErrorCode::InvalidCommand),
        _ => panic!("expected invalid_command, got {:?}", reply),
    }

    // Send without req_id
    let send = CommandEvent {
        client_id: 200,
        command: IpcCommand::Send {
            to: "ed25519.x".to_string(),
            kind: MessageKind::Notify,
            payload: json!({}),
            ref_id: None,
            req_id: None,
        },
    };
    let reply = server.handle_command(send).await.unwrap();
    match reply {
        DaemonReply::Error { error, .. } => assert_eq!(error, IpcErrorCode::InvalidCommand),
        _ => panic!("expected invalid_command, got {:?}", reply),
    }
}

// =========================================================================
// Test 3: req_id echo (Issue #2)
// =========================================================================

/// v2 `whoami` with `req_id` must include it in response.
#[tokio::test]
async fn v2_response_echoes_req_id() {
    let token = "a".repeat(64);
    let (server, _rx) = setup_server_with_token(&token).await;
    hello_and_auth(&server, 300, &token).await;

    let whoami = CommandEvent {
        client_id: 300,
        command: IpcCommand::Whoami {
            req_id: Some("my-req-42".to_string()),
        },
    };
    let reply = server.handle_command(whoami).await.unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["req_id"], "my-req-42");
}

// =========================================================================
// Test 4: buffer_size=0 disables buffering (Issue #3)
// =========================================================================

#[tokio::test]
async fn buffer_size_zero_inbox_always_empty() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let token = "a".repeat(64);
    let config = IpcServerConfig {
        buffer_size: 0,
        token: Some(token.clone()),
        ..Default::default()
    };
    let path = socket_path.clone();
    std::mem::forget(dir);
    let (server, _rx) = IpcServer::bind(path, 64, config).await.expect("bind");

    let envelope = axon::message::Envelope::new(
        "ed25519.sender".to_string(),
        "ed25519.receiver".to_string(),
        MessageKind::Notify,
        json!({"topic": "test", "data": {}}),
    );
    server.broadcast_inbound(&envelope).await.unwrap();

    // Hello + auth (direct handle_command — no peer cred auth)
    hello_and_auth(&server, 400, &token).await;

    let inbox = CommandEvent {
        client_id: 400,
        command: IpcCommand::Inbox {
            limit: 10,
            kinds: None,
            req_id: Some("i1".to_string()),
        },
    };
    let reply = server.handle_command(inbox).await.unwrap();
    match reply {
        DaemonReply::Inbox { messages, .. } => assert_eq!(messages.len(), 0),
        _ => panic!("expected Inbox reply, got {:?}", reply),
    }
}

// =========================================================================
// Test 5: Hardened mode (Issue #5)
// =========================================================================

/// Hardened mode: non-hello command before hello → `hello_required`.
#[tokio::test]
async fn hardened_mode_non_hello_returns_hello_required() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        allow_v1: false,
        ..Default::default()
    };
    let path = socket_path.clone();
    std::mem::forget(dir);
    let (server, _rx) = IpcServer::bind(path, 64, config).await.expect("bind");

    let peers = CommandEvent {
        client_id: 500,
        command: IpcCommand::Peers { req_id: None },
    };
    let reply = server.handle_command(peers).await.unwrap();
    match reply {
        DaemonReply::Error { error, .. } => assert_eq!(error, IpcErrorCode::HelloRequired),
        _ => panic!("expected hello_required, got {:?}", reply),
    }
}

/// Hardened mode: hello with version 1 → `unsupported_version`.
#[tokio::test]
async fn hardened_mode_v1_hello_returns_unsupported_version() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        allow_v1: false,
        ..Default::default()
    };
    let path = socket_path.clone();
    std::mem::forget(dir);
    let (server, _rx) = IpcServer::bind(path, 64, config).await.expect("bind");

    let hello = CommandEvent {
        client_id: 600,
        command: IpcCommand::Hello {
            version: 1,
            req_id: Some("h1".to_string()),
            consumer: "default".to_string(),
        },
    };
    let reply = server.handle_command(hello).await.unwrap();
    match reply {
        DaemonReply::Error { error, .. } => assert_eq!(error, IpcErrorCode::UnsupportedVersion),
        _ => panic!("expected unsupported_version, got {:?}", reply),
    }
}

// =========================================================================
// Test 6: cancel.reason required (Issue #7)
// =========================================================================

#[test]
fn cancel_reason_is_required() {
    let result = serde_json::from_value::<axon::message::CancelPayload>(json!({}));
    assert!(result.is_err());
}

// =========================================================================
// Test 7: result.error serializes as null (Issue #8)
// =========================================================================

#[test]
fn result_error_serializes_as_null() {
    let result = axon::message::ResultPayload {
        status: axon::message::TaskStatus::Completed,
        outcome: "Done".to_string(),
        data: None,
        error: None,
    };
    let json = serde_json::to_value(&result).unwrap();
    assert!(json.get("error").is_some(), "error field must be present");
    assert!(json["error"].is_null(), "error must be null, not omitted");
}
