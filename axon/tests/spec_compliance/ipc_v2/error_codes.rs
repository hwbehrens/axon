//! §5 Error codes, §2.4 hardened mode, and §3.3 unknown kind rejection.

use super::*;

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
