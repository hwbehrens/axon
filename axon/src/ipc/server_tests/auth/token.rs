use super::super::*;

#[tokio::test]
async fn auth_empty_token() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config =
        IpcServerConfig::default().with_token(Some("test_token_1234567890abcdef".to_string()));
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Test auth command directly to avoid peer credential auto-auth
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello {
            version: 2,
            req_id: None,
            consumer: "default".to_string(),
        },
    };
    let _ = server
        .handle_command(hello_event)
        .await
        .expect("handle hello");

    // Send empty token
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth {
            token: String::new(),
            req_id: Some("a1".to_string()),
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error, .. } => {
            assert!(!ok);
            assert_eq!(error, IpcErrorCode::AuthFailed);
        }
        _ => panic!("Expected Error reply with auth_failed, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_oversized_token() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default().with_token(Some("ab".repeat(32)));
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Test auth command directly to avoid peer credential auto-auth
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello {
            version: 2,
            req_id: None,
            consumer: "default".to_string(),
        },
    };
    let _ = server
        .handle_command(hello_event)
        .await
        .expect("handle hello");

    // Send oversized token (1000 chars)
    let oversized = "a".repeat(1000);
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth {
            token: oversized,
            req_id: Some("a1".to_string()),
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error, .. } => {
            assert!(!ok);
            assert_eq!(error, IpcErrorCode::AuthFailed);
        }
        _ => panic!("Expected Error reply with auth_failed, got {:?}", reply),
    }
}

#[tokio::test]
async fn v2_client_send_without_auth_requires_auth() {
    // Test: v2 client (after hello) trying to send without auth should get auth_required
    // Use direct handle_command calls to avoid peer credential auto-auth
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default().with_token(Some("a".repeat(64)));
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Step 1: Hello handshake
    let hello_event = CommandEvent {
        client_id: 888,
        command: IpcCommand::Hello {
            version: 2,
            req_id: None,
            consumer: "default".to_string(),
        },
    };
    let _ = server
        .handle_command(hello_event)
        .await
        .expect("handle hello");

    // Step 2: Try to send without auth (v1 command from v2 client)
    let send_event = CommandEvent {
        client_id: 888,
        command: IpcCommand::Send {
            to: "ed25519.target".to_string(),
            kind: MessageKind::Notify,
            payload: json!({}),
            ref_id: None,
            req_id: Some("s1".to_string()),
        },
    };
    let reply = server
        .handle_command(send_event)
        .await
        .expect("handle send");

    match reply {
        DaemonReply::Error { ok, error, .. } => {
            assert!(!ok);
            assert_eq!(
                error,
                IpcErrorCode::AuthRequired,
                "v2 client should need auth for send"
            );
        }
        _ => panic!("Expected auth_required error, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_malformed_token_non_hex() {
    // Test: token with non-hex characters should be rejected
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default().with_token(Some("a".repeat(64)));
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Token with invalid characters (not hex)
    let malformed = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"; // 64 chars but not hex
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth {
            token: malformed.to_string(),
            req_id: None,
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error, .. } => {
            assert!(!ok);
            assert_eq!(
                error,
                IpcErrorCode::AuthFailed,
                "non-hex token should be rejected"
            );
        }
        _ => panic!("Expected auth_failed error, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_malformed_token_wrong_length() {
    // Test: token with wrong length should be rejected
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default().with_token(Some("a".repeat(64)));
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Token with wrong length (32 chars instead of 64)
    let malformed = "a".repeat(32);
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth {
            token: malformed,
            req_id: None,
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error, .. } => {
            assert!(!ok);
            assert_eq!(
                error,
                IpcErrorCode::AuthFailed,
                "wrong-length token should be rejected"
            );
        }
        _ => panic!("Expected auth_failed error, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_fails_closed_when_no_token_configured() {
    // When config.token is None (no token file), token auth should be rejected.
    // Only peer-credential auth works in this mode.
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig::default(); // No token configured
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Hello first
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello {
            version: 2,
            req_id: None,
            consumer: "default".to_string(),
        },
    };
    let _ = server
        .handle_command(hello_event)
        .await
        .expect("handle hello");

    // Try auth with any token — should fail because no token is configured
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth {
            token: "a".repeat(64),
            req_id: Some("a1".to_string()),
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error, .. } => {
            assert!(!ok);
            assert_eq!(error, IpcErrorCode::AuthFailed);
        }
        _ => panic!("Expected auth_failed error, got {:?}", reply),
    }
}
