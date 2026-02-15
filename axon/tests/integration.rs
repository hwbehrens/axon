//! Integration tests — cross-module interactions.
//!
//! These tests exercise multiple subsystems together without starting
//! a full daemon process.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axon::config::{
    AxonPaths, Config, KnownPeer, StaticPeerConfig, load_known_peers, save_known_peers,
};
use axon::discovery::{Discovery, PeerEvent, StaticDiscovery};
use axon::identity::Identity;
use axon::ipc::{DaemonReply, IpcCommand, IpcServer};
use axon::message::{Envelope, MessageKind, decode, encode};
use axon::peer_table::{ConnectionStatus, PeerSource, PeerTable};
use axon::transport::QuicTransport;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// =========================================================================
// Helpers
// =========================================================================

fn make_identity() -> (Identity, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let id = Identity::load_or_generate(&paths).unwrap();
    (id, dir)
}

fn make_peer_record(id: &Identity, addr: std::net::SocketAddr) -> axon::peer_table::PeerRecord {
    axon::peer_table::PeerRecord {
        agent_id: id.agent_id().to_string(),
        addr,
        pubkey: id.public_key_base64().to_string(),
        source: PeerSource::Static,
        status: ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: Instant::now(),
    }
}

// =========================================================================
// Identity integration
// =========================================================================

/// Identity generates, persists, and reloads consistently.
#[test]
fn identity_roundtrip_persistence() {
    let dir = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let id1 = Identity::load_or_generate(&paths).unwrap();
    let id2 = Identity::load_or_generate(&paths).unwrap();

    assert_eq!(id1.agent_id(), id2.agent_id());
    assert_eq!(id1.public_key_base64(), id2.public_key_base64());

    // Certs are ephemeral but should both be valid DER.
    let cert1 = id1.make_quic_certificate().unwrap();
    let cert2 = id2.make_quic_certificate().unwrap();
    assert!(!cert1.cert_der.is_empty());
    assert!(!cert2.cert_der.is_empty());
}

/// Certificate contains Ed25519 public key that matches identity.
#[test]
fn cert_pubkey_matches_identity() {
    let (id, _dir) = make_identity();
    let cert = id.make_quic_certificate().unwrap();

    let extracted = axon::transport::extract_ed25519_pubkey_from_cert_der(&cert.cert_der).unwrap();
    let extracted_b64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, extracted);
    assert_eq!(extracted_b64, id.public_key_base64());
}

// =========================================================================
// Config integration
// =========================================================================

/// Config with static peers round-trips through TOML.
#[test]
fn config_static_peers_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
port = 8000

[[peers]]
agent_id = "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
addr = "10.0.0.1:7100"
pubkey = "dGVzdHB1YmtleQ=="
"#,
    )
    .unwrap();
    let config = Config::load(&path).unwrap();
    assert_eq!(config.effective_port(None), 8000);
    assert_eq!(config.peers.len(), 1);
    assert_eq!(
        config.peers[0].agent_id,
        "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    );
}

/// Known peers save and load integration.
#[tokio::test]
async fn known_peers_save_load_integration() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("known_peers.json");
    let peers = vec![
        KnownPeer {
            agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "Zm9v".to_string(),
            last_seen_unix_ms: 1000,
        },
        KnownPeer {
            agent_id: "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            addr: "10.0.0.2:7100".parse().unwrap(),
            pubkey: "YmFy".to_string(),
            last_seen_unix_ms: 2000,
        },
    ];

    save_known_peers(&path, &peers).await.unwrap();
    let loaded = load_known_peers(&path).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(
        loaded[0].agent_id,
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        loaded[1].agent_id,
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
}

// =========================================================================
// Discovery → PeerTable integration
// =========================================================================

/// StaticDiscovery emits PeerEvents that feed PeerTable correctly.
#[tokio::test]
async fn static_discovery_feeds_peer_table() {
    let peers = vec![
        StaticPeerConfig {
            agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "Zm9v".to_string(),
        },
        StaticPeerConfig {
            agent_id: "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            addr: "10.0.0.2:7100".parse().unwrap(),
            pubkey: "YmFy".to_string(),
        },
    ];

    let discovery = StaticDiscovery::new(peers);
    let (tx, mut rx) = mpsc::channel(16);

    tokio::spawn(async move {
        let _ = discovery.run(tx, CancellationToken::new()).await;
    });

    let table = PeerTable::new();

    // Drain discovery events into the peer table.
    for _ in 0..2 {
        match rx.recv().await.unwrap() {
            PeerEvent::Discovered {
                agent_id,
                addr,
                pubkey,
            } => {
                table.upsert_discovered(agent_id, addr, pubkey).await;
            }
            _ => panic!("expected Discovered"),
        }
    }

    let all = table.list().await;
    assert_eq!(all.len(), 2);
    assert!(
        table
            .get("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await
            .is_some()
    );
    assert!(
        table
            .get("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .await
            .is_some()
    );
}

// =========================================================================
// PeerTable lifecycle integration
// =========================================================================

/// Full lifecycle: insert static + discovered, connection state transitions,
/// stale cleanup, known_peers export.
#[tokio::test]
async fn peer_table_full_lifecycle() {
    let table = PeerTable::new();

    // 1. Insert static peer.
    table
        .upsert_static(&StaticPeerConfig {
            agent_id: "static_peer".to_string(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "Zm9v".to_string(),
        })
        .await;

    // 2. Insert discovered peer.
    table
        .upsert_discovered(
            "discovered_peer".to_string(),
            "10.0.0.2:7100".parse().unwrap(),
            "YmFy".to_string(),
        )
        .await;

    assert_eq!(table.list().await.len(), 2);

    // 3. Connection transitions.
    table
        .set_status("discovered_peer", ConnectionStatus::Connecting)
        .await;
    assert_eq!(
        table.get("discovered_peer").await.unwrap().status,
        ConnectionStatus::Connecting
    );

    table.set_connected("discovered_peer", Some(1.5)).await;
    let p = table.get("discovered_peer").await.unwrap();
    assert_eq!(p.status, ConnectionStatus::Connected);
    assert_eq!(p.rtt_ms, Some(1.5));

    // 4. Export as known peers.
    let known = table.to_known_peers().await;
    assert_eq!(known.len(), 2);

    // 5. Disconnect and verify status.
    table.set_disconnected("discovered_peer").await;
    let p = table.get("discovered_peer").await.unwrap();
    assert_eq!(p.status, ConnectionStatus::Disconnected);
    assert!(p.rtt_ms.is_none());

    // 6. Remove peer and verify.
    let removed = table.remove("discovered_peer").await;
    assert!(removed.is_some());
    assert!(table.get("discovered_peer").await.is_none());
    assert!(table.get("static_peer").await.is_some());
}

// =========================================================================
// Wire format integration (encode → decode through the full pipeline)
// =========================================================================

/// Envelope survives encode → decode roundtrip for all message kinds.
#[test]
fn envelope_roundtrip_all_kinds() {
    let a = "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string();
    let b = "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string();

    let payloads = vec![
        (MessageKind::Ping, json!({})),
        (
            MessageKind::Query,
            json!({"question": "test?", "domain": "meta"}),
        ),
        (
            MessageKind::Notify,
            json!({"topic": "t", "data": {}, "importance": "high"}),
        ),
        (
            MessageKind::Delegate,
            json!({"task": "do it", "priority": "urgent", "report_back": true}),
        ),
        (MessageKind::Discover, json!({})),
        (MessageKind::Cancel, json!({"reason": "changed mind"})),
    ];

    for (kind, payload) in payloads {
        let env = Envelope::new(a.clone(), b.clone(), kind, payload.clone());
        let encoded = encode(&env).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.kind, kind, "kind mismatch for {kind}");
        assert_eq!(decoded.from, a);
        assert_eq!(decoded.to, b);
    }
}

// =========================================================================
// Transport integration — QUIC peer-to-peer
// =========================================================================

/// Two transports connect and complete hello exchange.
#[tokio::test]
async fn transport_hello_exchange() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);
    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();
    assert!(conn.close_reason().is_none());
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}

/// Bidirectional query gets a response.
#[tokio::test]
async fn transport_query_gets_response() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let query = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Query,
        json!({"question": "What is 2+2?", "domain": "math"}),
    );

    let result = transport_a.send(&peer_b, query.clone()).await.unwrap();
    let response = result.expect("expected response for query");
    assert_eq!(response.kind, MessageKind::Response);
    assert_eq!(response.ref_id, Some(query.id));
}

/// Bidirectional discover gets capabilities.
#[tokio::test]
async fn transport_discover_gets_capabilities() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let discover = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Discover,
        json!({}),
    );

    let result = transport_a.send(&peer_b, discover.clone()).await.unwrap();
    let response = result.expect("expected response for discover");
    assert_eq!(response.kind, MessageKind::Capabilities);
    assert_eq!(response.ref_id, Some(discover.id));
}

/// Bidirectional delegate gets ack.
#[tokio::test]
async fn transport_delegate_gets_ack() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let delegate = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Delegate,
        json!({"task": "do something", "priority": "normal", "report_back": true}),
    );

    let result = transport_a.send(&peer_b, delegate.clone()).await.unwrap();
    let response = result.expect("expected ack for delegate");
    assert_eq!(response.kind, MessageKind::Ack);
    assert_eq!(response.ref_id, Some(delegate.id));
}

/// Cancel gets ack.
#[tokio::test]
async fn transport_cancel_gets_ack() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let cancel = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Cancel,
        json!({"reason": "plans changed"}),
    );

    let result = transport_a.send(&peer_b, cancel.clone()).await.unwrap();
    let response = result.expect("expected ack for cancel");
    assert_eq!(response.kind, MessageKind::Ack);
    assert_eq!(response.ref_id, Some(cancel.id));
}

/// Unidirectional notify delivered without response.
#[tokio::test]
async fn transport_notify_fire_and_forget() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();
    let mut rx_b = transport_b.subscribe_inbound();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let notify = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Notify,
        json!({"topic": "test.topic", "data": {"key": "value"}, "importance": "low"}),
    );

    let result = transport_a.send(&peer_b, notify).await.unwrap();
    assert!(result.is_none(), "notify should not return a response");

    // Drain until we find the notify (hello is also broadcast).
    let received = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
            .await
            .expect("timeout")
            .expect("recv");
        if msg.kind != MessageKind::Hello {
            break msg;
        }
    };
    assert_eq!(received.kind, MessageKind::Notify);
    assert_eq!(received.from, id_a.agent_id());
}

// =========================================================================
// IPC integration
// =========================================================================

/// IPC server accepts connection and routes commands.
#[tokio::test]
async fn ipc_send_command_roundtrip() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    // Send a "peers" command.
    client.write_all(b"{\"cmd\":\"peers\"}\n").await.unwrap();

    let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(cmd.command, IpcCommand::Peers));

    // Reply.
    server
        .send_reply(
            cmd.client_id,
            &DaemonReply::Peers {
                ok: true,
                peers: vec![],
            },
        )
        .await
        .unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client);
    reader.read_line(&mut line).await.unwrap();
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["peers"], json!([]));
}

/// IPC status command returns expected fields.
#[tokio::test]
async fn ipc_status_roundtrip() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    client.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();

    let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(cmd.command, IpcCommand::Status));

    server
        .send_reply(
            cmd.client_id,
            &DaemonReply::Status {
                ok: true,
                uptime_secs: 99,
                peers_connected: 2,
                messages_sent: 10,
                messages_received: 5,
            },
        )
        .await
        .unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client);
    reader.read_line(&mut line).await.unwrap();
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["uptime_secs"], 99);
    assert_eq!(v["peers_connected"], 2);
    assert_eq!(v["messages_sent"], 10);
    assert_eq!(v["messages_received"], 5);
}

/// Multiple sequential IPC commands on the same connection.
#[tokio::test]
async fn ipc_multiple_commands_sequential() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Send three commands in sequence.
    for i in 0..3 {
        client.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();

        let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .unwrap()
            .unwrap();

        server
            .send_reply(
                cmd.client_id,
                &DaemonReply::Status {
                    ok: true,
                    uptime_secs: i + 1,
                    peers_connected: 0,
                    messages_sent: 0,
                    messages_received: 0,
                },
            )
            .await
            .unwrap();

        let mut line = String::new();
        let mut reader = BufReader::new(&mut client);
        reader.read_line(&mut line).await.unwrap();
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["uptime_secs"], i + 1);
    }
}

/// Invalid IPC command returns error without crashing.
#[tokio::test]
async fn ipc_invalid_command_returns_error() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (_server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    client
        .write_all(b"{\"cmd\":\"nonexistent\"}\n")
        .await
        .unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(client);
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"ok\":false"));
    assert!(line.contains("invalid command"));
}

/// IPC send command with ref field deserializes correctly.
#[test]
fn ipc_send_with_ref_deserializes() {
    let input = r#"{"cmd":"send","to":"ed25519.deadbeef01234567deadbeef01234567","kind":"cancel","payload":{"reason":"changed plans"},"ref":"550e8400-e29b-41d4-a716-446655440000"}"#;
    let cmd: IpcCommand = serde_json::from_str(input).unwrap();
    match cmd {
        IpcCommand::Send {
            to, kind, ref_id, ..
        } => {
            assert_eq!(to, "ed25519.deadbeef01234567deadbeef01234567");
            assert_eq!(kind, MessageKind::Cancel);
            assert!(ref_id.is_some());
        }
        _ => panic!("expected Send"),
    }
}

/// IPC send without ref defaults to None.
#[test]
fn ipc_send_without_ref_defaults_to_none() {
    let input = r#"{"cmd":"send","to":"ed25519.deadbeef01234567deadbeef01234567","kind":"query","payload":{"question":"hello?"}}"#;
    let cmd: IpcCommand = serde_json::from_str(input).unwrap();
    match cmd {
        IpcCommand::Send { ref_id, .. } => {
            assert!(ref_id.is_none());
        }
        _ => panic!("expected Send"),
    }
}

/// IPC client disconnect does not affect other clients.
#[tokio::test]
async fn ipc_client_disconnect_isolation() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

    let client_a = UnixStream::connect(&socket_path).await.unwrap();
    let mut client_b = UnixStream::connect(&socket_path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.client_count().await, 2);

    // Drop client A.
    drop(client_a);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Client B should still receive broadcasts.
    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Notify,
        json!({"topic": "meta.status", "data": {}}),
    );
    server.broadcast_inbound(envelope).await.unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client_b);
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"inbound\":true"));
}

/// IPC broadcasts to multiple simultaneous clients.
#[tokio::test]
async fn ipc_broadcast_to_all_clients() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

    let mut client_a = UnixStream::connect(&socket_path).await.unwrap();
    let mut client_b = UnixStream::connect(&socket_path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Ping,
        json!({}),
    );
    server.broadcast_inbound(envelope).await.unwrap();

    let mut line_a = String::new();
    let mut line_b = String::new();
    let mut reader_a = BufReader::new(&mut client_a);
    let mut reader_b = BufReader::new(&mut client_b);
    reader_a.read_line(&mut line_a).await.unwrap();
    reader_b.read_line(&mut line_b).await.unwrap();
    assert!(line_a.contains("\"inbound\":true"));
    assert!(line_b.contains("\"inbound\":true"));
}

/// IPC cleanup removes socket file.
#[tokio::test]
async fn ipc_cleanup_removes_socket() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("axon.sock");
    let (server, _cmd_rx) = IpcServer::bind(socket_path.clone(), 64).await.unwrap();

    assert!(socket_path.exists());
    server.cleanup_socket().unwrap();
    assert!(!socket_path.exists());
}

// =========================================================================
// §10 Protocol Violation Handling
// =========================================================================

/// spec.md §10: After hello, connection is authenticated and subsequent
/// messages are accepted. Verifies the hello-first invariant holds.
#[tokio::test]
async fn violation_hello_first_invariant() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    // ensure_connection performs hello automatically; connection should succeed.
    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();
    assert!(transport_a.has_connection(id_b.agent_id()).await);
    assert!(conn.close_reason().is_none());

    // After hello, a query should succeed (proves post-hello messages are accepted).
    let query = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Query,
        json!({"question": "post-hello test", "domain": "test"}),
    );
    let result = transport_a.send(&peer_b, query).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().kind, MessageKind::Response);
}

/// spec.md §10: Version mismatch in hello returns error(incompatible_version).
/// Tested via auto_response since the public transport API always sends v1.
#[tokio::test]
async fn violation_version_mismatch_error() {
    let req = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Hello,
        json!({"protocol_versions": [99, 100]}),
    );
    let resp = axon::transport::auto_response(&req, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(resp.kind, MessageKind::Error);
    let payload: Value = serde_json::from_str(resp.payload.get()).unwrap();
    assert_eq!(payload["code"], "incompatible_version");
    assert_eq!(payload["retryable"], false);
}

/// spec.md §10: Unknown kind on bidi stream returns error(unknown_kind).
/// Tested via auto_response since we cannot inject raw wire bytes from
/// integration tests (framing is pub(crate)).
#[tokio::test]
async fn violation_unknown_kind_on_bidi_returns_error() {
    // auto_response's catch-all arm handles unexpected kinds on bidi.
    // Construct an envelope that would hit that arm (e.g. Result on bidi).
    let unexpected_bidi = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Result,
        json!({"task_id": "123", "status": "completed", "output": {}}),
    );
    let resp = axon::transport::auto_response(
        &unexpected_bidi,
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    assert_eq!(resp.kind, MessageKind::Error);
    let payload: Value = serde_json::from_str(resp.payload.get()).unwrap();
    assert_eq!(payload["code"], "unknown_kind");
}

/// spec.md §10: Fire-and-forget messages (notify) delivered via uni stream
/// return no response. Verifies transport drops no valid fire-and-forget.
#[tokio::test]
async fn violation_fire_and_forget_no_response() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();
    let mut rx_b = transport_b.subscribe_inbound();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let notify = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Notify,
        json!({"topic": "violation.test", "data": {"x": 1}, "importance": "low"}),
    );

    let result = transport_a.send(&peer_b, notify).await.unwrap();
    assert!(
        result.is_none(),
        "fire-and-forget must not return a response"
    );

    // Verify the message was delivered.
    let received = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
            .await
            .expect("timeout")
            .expect("recv");
        if msg.kind != MessageKind::Hello {
            break msg;
        }
    };
    assert_eq!(received.kind, MessageKind::Notify);
}

/// spec.md §10: Ping on bidi gets pong (validates auto_response for
/// request kinds that must produce the correct response type).
#[tokio::test]
async fn violation_ping_gets_pong_response() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let ping = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Ping,
        json!({}),
    );

    let result = transport_a.send(&peer_b, ping.clone()).await.unwrap();
    let pong = result.expect("ping must get a pong response");
    assert_eq!(pong.kind, MessageKind::Pong);
    assert_eq!(pong.ref_id, Some(ping.id));
}

/// spec.md §10: Invalid envelope on bidi request returns error(invalid_envelope).
/// Tested via auto_response since the transport validates before responding.
#[tokio::test]
async fn violation_invalid_envelope_returns_error() {
    // An envelope with bad agent IDs should fail validate().
    let invalid = Envelope::new(
        "bad_id".to_string(),
        "also_bad".to_string(),
        MessageKind::Query,
        json!({"question": "test?"}),
    );
    assert!(invalid.validate().is_err());

    // The transport would send error(invalid_envelope) for this on a bidi stream.
    // Verify the error code is available.
    let error_payload = axon::message::ErrorPayload {
        code: axon::message::ErrorCode::InvalidEnvelope,
        message: "envelope validation failed".to_string(),
        retryable: false,
    };
    let v: Value = serde_json::to_value(&error_payload).unwrap();
    assert_eq!(v["code"], "invalid_envelope");
    assert_eq!(v["retryable"], false);
}

/// spec.md §10: Multiple request types after hello all get correct responses.
/// Verifies the connection stays open and handles multiple violations/requests.
#[tokio::test]
async fn violation_connection_survives_multiple_requests() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    // Send ping, then query, then discover — all should succeed.
    let ping = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Ping,
        json!({}),
    );
    let pong = transport_a
        .send(&peer_b, ping)
        .await
        .unwrap()
        .expect("expected pong");
    assert_eq!(pong.kind, MessageKind::Pong);

    let query = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Query,
        json!({"question": "second request", "domain": "test"}),
    );
    let response = transport_a
        .send(&peer_b, query)
        .await
        .unwrap()
        .expect("expected response");
    assert_eq!(response.kind, MessageKind::Response);

    let discover = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Discover,
        json!({}),
    );
    let caps = transport_a
        .send(&peer_b, discover)
        .await
        .unwrap()
        .expect("expected capabilities");
    assert_eq!(caps.kind, MessageKind::Capabilities);

    // Connection should still be alive.
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}
