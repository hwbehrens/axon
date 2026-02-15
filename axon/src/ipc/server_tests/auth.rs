use super::*;

#[tokio::test]
async fn hello_handshake() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        public_key: "test-key".to_string(),
        name: Some("test-daemon".to_string()),
        version: "0.1.0".to_string(),
        ..Default::default()
    };
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");

    client
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write");

    let cmd = tokio::time::timeout(std::time::Duration::from_secs(2), cmd_rx.recv())
        .await
        .expect("timeout")
        .expect("recv");

    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client);
    reader.read_line(&mut line).await.expect("read");

    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["version"], 2);
    assert_eq!(v["agent_id"], "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert!(!v["features"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn auth_with_token() {
    // Use direct handle_command calls to test token validation
    // without peer-credential auto-auth interfering
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let token = "ab".repeat(32); // Valid 64-char hex token
    let config = IpcServerConfig {
        agent_id: "ed25519.test".to_string(),
        public_key: "key".to_string(),
        token: Some(token.clone()),
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Step 1: Hello handshake
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello { version: 2 },
    };
    let reply = server
        .handle_command(hello_event)
        .await
        .expect("handle hello");
    match &reply {
        DaemonReply::Hello { ok, version, .. } => {
            assert!(ok);
            assert_eq!(*version, 2);
        }
        _ => panic!("Expected Hello reply, got {:?}", reply),
    }

    // Step 2: Auth with correct token
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth {
            token: token.clone(),
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");
    match reply {
        DaemonReply::Auth { ok, auth } => {
            assert!(ok);
            assert_eq!(auth, "accepted");
        }
        _ => panic!("Expected Auth reply, got {:?}", reply),
    }

    // Step 3: Verify authenticated client can use v2 commands
    let inbox_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Inbox {
            limit: 10,
            since: None,
            kinds: None,
        },
    };
    let reply = server
        .handle_command(inbox_event)
        .await
        .expect("handle inbox");
    match reply {
        DaemonReply::Inbox { ok, .. } => assert!(ok),
        _ => panic!("Expected Inbox reply, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_with_peer_credentials_succeeds_even_with_wrong_token() {
    // Note: On supported platforms (Linux/macOS), peer credentials
    // authenticate the connection automatically, so even a wrong token
    // will be accepted (peer creds take precedence).
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        agent_id: "ed25519.test".to_string(),
        public_key: "key".to_string(),
        token: Some("ab".repeat(32)),
        ..Default::default()
    };
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let client = UnixStream::connect(&socket_path).await.expect("connect");
    let (read_half, mut write_half) = client.into_split();
    let mut reader = BufReader::new(read_half);

    // Send hello
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write");

    let cmd = cmd_rx.recv().await.expect("recv hello");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read");

    // Send auth with wrong token - but peer credentials will accept it
    write_half
        .write_all(b"{\"cmd\":\"auth\",\"token\":\"wrong\"}\n")
        .await
        .expect("write");

    let cmd = cmd_rx.recv().await.expect("recv auth");
    let reply = server.handle_command(cmd).await.expect("handle");
    server.send_reply(1, &reply).await.expect("send reply");

    line.clear();
    reader.read_line(&mut line).await.expect("read");

    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    // On supported platforms with peer credentials, this will succeed
    // even with wrong token because peer credentials take precedence
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert_eq!(v["ok"], true);

    // On unsupported platforms, token validation would fail
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    assert_eq!(v["ok"], false);
}

#[tokio::test]
async fn whoami_returns_identity() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        agent_id: "ed25519.test123".to_string(),
        public_key: "pubkey123".to_string(),
        name: Some("test-agent".to_string()),
        version: "1.2.3".to_string(),
        ..Default::default()
    };
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    let client = UnixStream::connect(&socket_path).await.expect("connect");
    let (read_half, mut write_half) = client.into_split();
    let mut reader = BufReader::new(read_half);

    // Send hello first (required for v2 commands)
    write_half
        .write_all(b"{\"cmd\":\"hello\",\"version\":2}\n")
        .await
        .expect("write hello");

    let cmd = cmd_rx.recv().await.expect("recv hello");
    let reply = server.handle_command(cmd).await.expect("handle hello");
    server
        .send_reply(1, &reply)
        .await
        .expect("send hello reply");

    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read hello reply");

    // Now send whoami
    write_half
        .write_all(b"{\"cmd\":\"whoami\"}\n")
        .await
        .expect("write whoami");

    let cmd = cmd_rx.recv().await.expect("recv whoami");
    let reply = server.handle_command(cmd).await.expect("handle whoami");
    server
        .send_reply(1, &reply)
        .await
        .expect("send whoami reply");

    line.clear();
    reader
        .read_line(&mut line)
        .await
        .expect("read whoami reply");

    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["agent_id"], "ed25519.test123");
    assert_eq!(v["public_key"], "pubkey123");
    assert_eq!(v["name"], "test-agent");
    assert_eq!(v["version"], "1.2.3");
    assert_eq!(v["ipc_version"], 2);
}

#[tokio::test]
async fn auth_gating_v2_client_needs_auth() {
    // This test verifies the auth gating logic by directly testing handle_command
    // without going through the socket (which would auto-authenticate via peer creds)
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        token: Some("test_token_1234567890abcdef".to_string()),
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Simulate a v2 client that has done hello but not auth
    // First send hello to establish v2 protocol
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello { version: 2 },
    };
    let _ = server
        .handle_command(hello_event)
        .await
        .expect("handle hello");

    // Now try inbox without being authenticated
    // (Since we didn't go through the socket, no peer cred auto-auth happened)
    let inbox_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Inbox {
            limit: 10,
            since: None,
            kinds: None,
        },
    };
    let reply = server
        .handle_command(inbox_event)
        .await
        .expect("handle inbox");

    // Should get auth_required error
    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(error, "auth_required");
        }
        _ => panic!("Expected Error reply, got {:?}", reply),
    }
}

#[tokio::test]
async fn v2_command_without_hello_returns_hello_required() {
    // v2 commands (inbox, whoami, ack, subscribe) sent without hello
    // should return hello_required error
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        token: Some("a".repeat(64)),
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Try inbox without hello
    let inbox_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Inbox {
            limit: 10,
            since: None,
            kinds: None,
        },
    };
    let reply = server.handle_command(inbox_event).await.expect("handle");
    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(error, "hello_required");
        }
        _ => panic!("Expected hello_required error, got {:?}", reply),
    }

    // Try whoami without hello
    let whoami_event = CommandEvent {
        client_id: 1000,
        command: IpcCommand::Whoami,
    };
    let reply = server.handle_command(whoami_event).await.expect("handle");
    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(error, "hello_required");
        }
        _ => panic!("Expected hello_required error, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_empty_token() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        token: Some("test_token_1234567890abcdef".to_string()),
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Test auth command directly to avoid peer credential auto-auth
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello { version: 2 },
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
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(error, "auth_failed");
        }
        _ => panic!("Expected Error reply with auth_failed, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_oversized_token() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        token: Some("ab".repeat(32)),
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Test auth command directly to avoid peer credential auto-auth
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello { version: 2 },
    };
    let _ = server
        .handle_command(hello_event)
        .await
        .expect("handle hello");

    // Send oversized token (1000 chars)
    let oversized = "a".repeat(1000);
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth { token: oversized },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(error, "auth_failed");
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
    let config = IpcServerConfig {
        token: Some("a".repeat(64)), // Valid 64-char token
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Step 1: Hello handshake
    let hello_event = CommandEvent {
        client_id: 888,
        command: IpcCommand::Hello { version: 2 },
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
        },
    };
    let reply = server
        .handle_command(send_event)
        .await
        .expect("handle send");

    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(
                error, "auth_required",
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
    let config = IpcServerConfig {
        token: Some("a".repeat(64)), // Valid 64-char hex token
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Token with invalid characters (not hex)
    let malformed = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"; // 64 chars but not hex
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth {
            token: malformed.to_string(),
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(error, "auth_failed", "non-hex token should be rejected");
        }
        _ => panic!("Expected auth_failed error, got {:?}", reply),
    }
}

#[tokio::test]
async fn auth_malformed_token_wrong_length() {
    // Test: token with wrong length should be rejected
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let config = IpcServerConfig {
        token: Some("a".repeat(64)), // Valid 64-char hex token
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Token with wrong length (32 chars instead of 64)
    let malformed = "a".repeat(32);
    let auth_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Auth { token: malformed },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(
                error, "auth_failed",
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
    let config = IpcServerConfig {
        token: None, // No token configured
        ..Default::default()
    };
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64, config)
        .await
        .expect("bind IPC server");

    // Hello first
    let hello_event = CommandEvent {
        client_id: 999,
        command: IpcCommand::Hello { version: 2 },
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
        },
    };
    let reply = server
        .handle_command(auth_event)
        .await
        .expect("handle auth");

    match reply {
        DaemonReply::Error { ok, error } => {
            assert!(!ok);
            assert_eq!(error, "auth_failed");
        }
        _ => panic!("Expected auth_failed error, got {:?}", reply),
    }
}
