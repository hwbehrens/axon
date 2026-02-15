//! Adversarial and stress tests.
//!
//! These tests exercise the system under hostile inputs, concurrent
//! contention, and boundary conditions to verify resilience.

use std::time::Duration;

use axon::config::{Config, KnownPeer, load_known_peers, save_known_peers};
use axon::ipc::{DaemonReply, IpcServer};
use axon::message::{Envelope, MAX_MESSAGE_SIZE, MessageKind, PROTOCOL_VERSION, decode, encode};
use axon::peer_table::{ConnectionStatus, PeerTable};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

// =========================================================================
// Helpers
// =========================================================================

fn agent_a() -> String {
    "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8".to_string()
}

fn agent_b() -> String {
    "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

fn random_agent_ids(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("ed25519.{:0>32x}", i)).collect()
}

// =========================================================================
// §1 Concurrent IPC flood
// =========================================================================

/// 50 concurrent clients each send 10 rapid status commands.
/// Server must handle all without panics, deadlocks, or lost replies.
#[tokio::test]
async fn concurrent_ipc_flood() {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 128).await.unwrap();

        let num_clients = 50;
        let cmds_per_client = 10;

        // Spawn command handler — replies with Status for every command.
        let server_for_handler = server.clone();
        let handler = tokio::spawn(async move {
            let mut count = 0u64;
            while let Some(evt) = cmd_rx.recv().await {
                count += 1;
                let _ = server_for_handler
                    .send_reply(
                        evt.client_id,
                        &DaemonReply::Status {
                            ok: true,
                            uptime_secs: count,
                            peers_connected: 0,
                            messages_sent: 0,
                            messages_received: 0,
                        },
                    )
                    .await;
            }
            count
        });

        // Spawn 50 clients, each sending 10 commands.
        // Each client interleaves send/read to avoid overflowing the
        // internal command channel (bounded at 256).
        let mut client_handles = Vec::new();
        for _ in 0..num_clients {
            let path = socket_path.clone();
            client_handles.push(tokio::spawn(async move {
                let stream = UnixStream::connect(&path).await.unwrap();
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);

                let mut replies = 0;
                let mut line = String::new();
                for _ in 0..cmds_per_client {
                    write_half
                        .write_all(b"{\"cmd\":\"status\"}\n")
                        .await
                        .unwrap();
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    let v: Value = serde_json::from_str(line.trim()).unwrap();
                    assert_eq!(v["ok"], true, "reply should have ok=true");
                    replies += 1;
                }
                replies
            }));
        }

        let mut total_replies = 0;
        for handle in client_handles {
            let count = handle.await.unwrap();
            total_replies += count;
        }

        assert_eq!(
            total_replies,
            num_clients * cmds_per_client,
            "every client must receive all its replies"
        );

        // The accept loop and per-client tasks are spawned (not owned),
        // so the command channel won't close just from dropping the server.
        // Abort the handler to avoid blocking.
        handler.abort();
        drop(server);
    })
    .await;

    assert!(result.is_ok(), "concurrent_ipc_flood timed out (30s)");
}

// =========================================================================
// §2 Peer table contention
// =========================================================================

/// 50 tasks do random peer table operations concurrently.
/// Must complete without panics or deadlocks.
#[tokio::test]
async fn peer_table_contention() {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let table = PeerTable::new();
        let ids = random_agent_ids(20);

        let mut handles = Vec::new();
        for task_id in 0..50u32 {
            let table = table.clone();
            let ids = ids.clone();
            handles.push(tokio::spawn(async move {
                for iter in 0..20u32 {
                    let idx = ((task_id as usize).wrapping_mul(7) + iter as usize) % ids.len();
                    let id = &ids[idx];
                    let op = (task_id.wrapping_add(iter)) % 7;

                    match op {
                        0 => {
                            table
                                .upsert_discovered(
                                    id.clone(),
                                    "127.0.0.1:7100".parse().unwrap(),
                                    "cHVia2V5".to_string(),
                                )
                                .await;
                        }
                        1 => {
                            table.set_status(id, ConnectionStatus::Connecting).await;
                        }
                        2 => {
                            table.set_connected(id, Some(1.0)).await;
                        }
                        3 => {
                            table.set_disconnected(id).await;
                        }
                        4 => {
                            let _ = table.list().await;
                        }
                        5 => {
                            let _ = table.get(id).await;
                        }
                        6 => {
                            let _ = table.remove(id).await;
                        }
                        _ => unreachable!(),
                    }
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Consistency check: list and get must agree.
        let listed = table.list().await;
        for record in &listed {
            let got = table.get(&record.agent_id).await;
            assert!(
                got.is_some(),
                "listed peer {} must be found via get()",
                record.agent_id
            );
        }
    })
    .await;

    assert!(result.is_ok(), "peer_table_contention timed out (10s)");
}

// =========================================================================
// §3 IPC malformed input resilience
// =========================================================================

/// Server survives malformed IPC inputs and continues serving valid commands.
/// Note: non-UTF8 bytes cause tokio's `lines()` to return an I/O error,
/// which closes that connection. We test those on separate connections.
#[tokio::test]
async fn ipc_malformed_input_resilience() {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

        // Spawn handler for valid commands.
        let server_clone = server.clone();
        let handler = tokio::spawn(async move {
            while let Some(evt) = cmd_rx.recv().await {
                let _ = server_clone
                    .send_reply(
                        evt.client_id,
                        &DaemonReply::Status {
                            ok: true,
                            uptime_secs: 42,
                            peers_connected: 0,
                            messages_sent: 0,
                            messages_received: 0,
                        },
                    )
                    .await;
            }
        });

        // --- UTF-8–safe malformed inputs on a single connection ---
        {
            let stream = UnixStream::connect(&socket_path).await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let malformed_inputs: Vec<&[u8]> = vec![
                // Empty string
                b"\n",
                // Valid JSON but not a valid command
                b"{\"cmd\":\"nonexistent\"}\n",
                // Partial JSON
                b"{\"cmd\":\"sta\n",
                // Null bytes (valid UTF-8)
                b"\0\0\0\n",
            ];

            for input in &malformed_inputs {
                write_half.write_all(input).await.unwrap();
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                assert!(
                    line.contains("\"ok\":false"),
                    "malformed input should get error reply, got: {line}"
                );
            }

            // Extremely long line (100KB of 'a' chars).
            let long_line = format!("{}\n", "a".repeat(100_000));
            write_half.write_all(long_line.as_bytes()).await.unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            assert!(
                line.contains("\"ok\":false"),
                "long malformed input should get error reply, got: {line}"
            );

            // Connection should still be alive — send a valid command.
            write_half
                .write_all(b"{\"cmd\":\"status\"}\n")
                .await
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let v: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(
                v["ok"], true,
                "server must still respond after malformed input"
            );
        }

        // --- Non-UTF8 binary garbage on a separate connection ---
        // This closes the connection (tokio lines() rejects invalid UTF-8),
        // but must not crash the server.
        {
            let mut stream = UnixStream::connect(&socket_path).await.unwrap();
            stream.write_all(b"\x80\x81\x82\xff\xfe\n").await.unwrap();
            // Connection will be closed by the server; just wait for EOF.
            let mut buf = vec![0u8; 1024];
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
            )
            .await;
        }

        // --- Verify server is still alive after binary garbage killed a connection ---
        {
            let stream = UnixStream::connect(&socket_path).await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            write_half
                .write_all(b"{\"cmd\":\"status\"}\n")
                .await
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let v: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(
                v["ok"], true,
                "server must still work after binary garbage on another connection"
            );
        }

        drop(server);
        handler.abort();
    })
    .await;

    assert!(
        result.is_ok(),
        "ipc_malformed_input_resilience timed out (10s)"
    );
}

// =========================================================================
// §4 Envelope validation edge cases
// =========================================================================

/// Envelope::validate() rejects malformed agent IDs, zero versions, etc.
#[test]
fn envelope_validation_edge_cases() {
    // Uppercase hex — is_ascii_hexdigit accepts uppercase, so validate() passes.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.A1B2C3D4E5F6A7B8A1B2C3D4E5F6A7B8".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_ok(),
        "uppercase hex is valid per is_ascii_hexdigit"
    );

    // Non-hex characters in agent ID.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "non-hex characters should fail validation"
    );

    // Missing ed25519. prefix.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "agent ID without ed25519. prefix should fail validation"
    );

    // Hex part too short (31 hex chars).
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "31-char hex suffix should fail validation"
    );

    // Hex part too long (33 hex chars).
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b80".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "33-char hex suffix should fail validation"
    );

    // Version = 0.
    let env = Envelope {
        v: 0,
        id: uuid::Uuid::new_v4(),
        from: agent_a().into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(env.validate().is_err(), "version 0 should fail validation");

    // Timestamp = 0.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: agent_a().into(),
        to: agent_b().into(),
        ts: 0,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "timestamp 0 should fail validation"
    );

    // Empty from string.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: String::new().into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(env.validate().is_err(), "empty from should fail validation");

    // Empty to string.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: agent_a().into(),
        to: String::new().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(env.validate().is_err(), "empty to should fail validation");

    // Unicode characters in agent IDs.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b✓".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "unicode in agent ID should fail validation"
    );
}

// =========================================================================
// §5 Wire format boundary conditions
// =========================================================================

/// Wire encode/decode with boundary sizes, truncated data, and wrong types.
#[test]
fn wire_format_boundary_conditions() {
    // Find a payload size that produces an envelope exactly at MAX_MESSAGE_SIZE.
    // We probe by binary-searching the payload string length.
    let mut low = 0usize;
    let mut high = MAX_MESSAGE_SIZE as usize;
    while low + 1 < high {
        let mid = (low + high) / 2;
        let payload_str = "x".repeat(mid);
        let env = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Query,
            json!({"question": payload_str}),
        );
        match encode(&env) {
            Ok(encoded) => {
                let body_len = encoded.len() - 4;
                if body_len <= MAX_MESSAGE_SIZE as usize {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            Err(_) => {
                high = mid;
            }
        }
    }

    // `low` is the largest payload string that fits. Verify it encodes.
    let payload_str = "x".repeat(low);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Query,
        json!({"question": payload_str}),
    );
    assert!(
        encode(&env).is_ok(),
        "envelope at MAX_MESSAGE_SIZE boundary should encode"
    );

    // One more char should fail.
    let payload_str_over = "x".repeat(low + 1);
    let env_over = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Query,
        json!({"question": payload_str_over}),
    );
    assert!(
        encode(&env_over).is_err(),
        "envelope exceeding MAX_MESSAGE_SIZE should fail"
    );

    // Decode with empty bytes.
    assert!(
        decode(b"").is_err(),
        "decoding empty bytes should return Err"
    );

    // Decode with single byte.
    assert!(
        decode(b"{").is_err(),
        "decoding single byte should return Err"
    );

    // Decode with 3 bytes.
    assert!(
        decode(b"abc").is_err(),
        "decoding 3 bytes should return Err"
    );

    // Valid JSON but missing required fields.
    let incomplete = br#"{"v":1,"id":"550e8400-e29b-41d4-a716-446655440000"}"#;
    assert!(
        decode(incomplete).is_err(),
        "JSON missing required fields should return Err"
    );

    // Valid JSON but wrong types (v as string instead of number).
    let wrong_types = br#"{"v":"one","id":"550e8400-e29b-41d4-a716-446655440000","from":"a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8","to":"f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","ts":1700000000000,"kind":"ping","payload":{}}"#;
    assert!(
        decode(wrong_types).is_err(),
        "JSON with wrong types should return Err"
    );
}

// =========================================================================
// §7 IPC client disconnect under load
// =========================================================================

/// Some clients disconnect abruptly while others keep working.
/// Surviving clients and broadcast_inbound must continue functioning.
#[tokio::test]
async fn ipc_client_disconnect_under_load() {
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

        // Spawn command handler.
        let server_for_handler = server.clone();
        let handler = tokio::spawn(async move {
            while let Some(evt) = cmd_rx.recv().await {
                let _ = server_for_handler
                    .send_reply(
                        evt.client_id,
                        &DaemonReply::Status {
                            ok: true,
                            uptime_secs: 1,
                            peers_connected: 0,
                            messages_sent: 0,
                            messages_received: 0,
                        },
                    )
                    .await;
            }
        });

        // Connect 10 clients — first 5 will disconnect, last 5 will survive.
        let mut disconnect_streams = Vec::new();
        let mut surviving_streams = Vec::new();

        for _ in 0..5 {
            disconnect_streams.push(UnixStream::connect(&socket_path).await.unwrap());
        }
        for _ in 0..5 {
            surviving_streams.push(UnixStream::connect(&socket_path).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            server.client_count().await,
            10,
            "all 10 clients should be connected"
        );

        // Abruptly disconnect 5 clients while surviving ones send commands.
        let send_task = {
            let socket_path = socket_path.clone();
            tokio::spawn(async move {
                // Give disconnect a moment.
                tokio::time::sleep(Duration::from_millis(10)).await;

                // Use the surviving streams (we pass them via a channel trick).
                // Actually, we need to send on the surviving streams. Let's
                // do it inline after dropping.
                socket_path // return for type
            })
        };

        // Drop the 5 disconnect clients abruptly.
        drop(disconnect_streams);
        send_task.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Surviving clients should still work.
        for stream in &mut surviving_streams {
            stream.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();
        }

        for stream in &mut surviving_streams {
            let mut line = String::new();
            let mut reader = BufReader::new(stream);
            reader.read_line(&mut line).await.unwrap();
            let v: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(v["ok"], true, "surviving client must receive reply");
        }

        // Broadcast should reach surviving clients.
        let envelope = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Notify,
            json!({"topic": "meta.status", "data": {}}),
        );
        server.broadcast_inbound(envelope).await.unwrap();

        for stream in &mut surviving_streams {
            let mut line = String::new();
            let mut reader = BufReader::new(stream);
            reader.read_line(&mut line).await.unwrap();
            assert!(
                line.contains("\"inbound\":true"),
                "surviving client must receive broadcast"
            );
        }

        drop(server);
        handler.abort();
    })
    .await;

    assert!(
        result.is_ok(),
        "ipc_client_disconnect_under_load timed out (15s)"
    );
}

// =========================================================================
// §8 Known peers corruption resilience
// =========================================================================

/// load_known_peers handles corrupt, truncated, and wrong-schema files
/// without panicking.
#[tokio::test]
async fn known_peers_corruption_resilience() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("known_peers.json");

    // Random bytes.
    std::fs::write(&path, b"\x80\x81\x82\xff random garbage").unwrap();
    assert!(
        load_known_peers(&path).is_err(),
        "random bytes should return Err"
    );

    // Truncated JSON.
    std::fs::write(&path, b"[{\"agent_id\":\"aaa").unwrap();
    assert!(
        load_known_peers(&path).is_err(),
        "truncated JSON should return Err"
    );

    // Wrong schema — array of strings instead of KnownPeer objects.
    std::fs::write(&path, b"[\"not\",\"a\",\"peer\"]").unwrap();
    assert!(
        load_known_peers(&path).is_err(),
        "wrong schema should return Err"
    );

    // Empty array — valid, should load as empty vec.
    std::fs::write(&path, b"[]").unwrap();
    let peers = load_known_peers(&path).unwrap();
    assert!(peers.is_empty(), "empty array should load as empty vec");

    // Valid data.
    let valid = vec![KnownPeer {
        agent_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        addr: "10.0.0.1:7100".parse().unwrap(),
        pubkey: "Zm9v".to_string(),
        last_seen_unix_ms: 1000,
    }];
    save_known_peers(&path, &valid).await.unwrap();
    let loaded = load_known_peers(&path).unwrap();
    assert_eq!(loaded.len(), 1, "valid data should load correctly");
    assert_eq!(loaded[0].agent_id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
}

// =========================================================================
// §9 Config corruption resilience
// =========================================================================

/// Config::load handles corrupt, invalid, and missing files gracefully.
#[test]
fn config_corruption_resilience() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    // Random bytes.
    std::fs::write(&path, b"\x80\x81\x82\xff random garbage").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "random bytes should return Err"
    );

    // Invalid TOML syntax.
    std::fs::write(&path, b"[invalid toml =====").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "invalid TOML should return Err"
    );

    // Valid TOML but wrong types (port as string).
    std::fs::write(&path, b"port = \"not a number\"").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "wrong types should return Err"
    );

    // Valid TOML but wrong nested type (peers as string).
    std::fs::write(&path, b"peers = \"not an array\"").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "wrong nested types should return Err"
    );

    // Non-existent path returns default config (not Err).
    let missing = dir.path().join("nonexistent.toml");
    let config = Config::load(&missing).unwrap();
    assert_eq!(
        config.effective_port(None),
        7100,
        "missing config should use default port"
    );
    assert!(
        config.peers.is_empty(),
        "missing config should have no peers"
    );

    // Valid minimal config.
    std::fs::write(&path, b"port = 9000").unwrap();
    let config = Config::load(&path).unwrap();
    assert_eq!(config.effective_port(None), 9000);
    assert!(config.peers.is_empty());
}
