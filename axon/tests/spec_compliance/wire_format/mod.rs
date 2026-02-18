use super::*;

mod ipc_shapes;
mod violations;

// =========================================================================
// §4 Wire format — FIN-delimited framing
// =========================================================================

/// `spec/WIRE_FORMAT.md` wire framing: raw JSON bytes (FIN-delimited, no length prefix).
#[test]
fn wire_format_is_raw_json() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let encoded = encode(&env).unwrap();

    let decoded: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded["kind"], "request");
}

/// spec/WIRE_FORMAT.md envelope schema: `from`/`to` are daemon-local and not on
/// QUIC wire payloads.
#[test]
fn wire_encoding_omits_from_and_to_fields() {
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"topic": "t"}),
    );
    let encoded = env.wire_encode().unwrap();
    let decoded: Value = serde_json::from_slice(&encoded).unwrap();
    assert!(decoded.get("from").is_none());
    assert!(decoded.get("to").is_none());
}

/// `spec/WIRE_FORMAT.md` limits: max message size is 64KB.
#[test]
fn max_message_size_is_64kb() {
    assert_eq!(MAX_MESSAGE_SIZE, 65536);
}

/// `spec/WIRE_FORMAT.md` limits: messages >64KB are rejected.
#[test]
fn oversized_message_rejected() {
    let big = "x".repeat(MAX_MESSAGE_SIZE as usize);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"question": big}),
    );
    let result = encode(&env);
    assert!(result.is_err());
}

/// encode/decode round-trip preserves all fields.
#[test]
fn encode_decode_full_roundtrip() {
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"question": "what is 2+2?", "domain": "math"}),
    );
    let encoded = encode(&env).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(env.id, decoded.id);
    assert_eq!(env.from, decoded.from);
    assert_eq!(env.to, decoded.to);
    assert_eq!(env.kind, decoded.kind);
    assert_eq!(env.payload.get(), decoded.payload.get());
}

// =========================================================================
// §1 Identity — agent ID derivation
// =========================================================================

/// `spec/SPEC.md` identity: Agent ID = "ed25519." + first 16 bytes of SHA-256(public key),
/// hex-encoded (40 chars total).
#[test]
fn agent_id_is_40_chars_with_prefix() {
    let paths = axon::config::AxonPaths::from_root(std::path::PathBuf::from(
        tempfile::tempdir().unwrap().path(),
    ));
    let identity = axon::identity::Identity::load_or_generate(&paths).unwrap();
    let id = identity.agent_id();
    assert_eq!(id.len(), 40);
    assert!(id.starts_with("ed25519."));
    assert!(id[8..].chars().all(|c| c.is_ascii_hexdigit()));
}

/// `spec/SPEC.md` identity: agent ID is deterministic from the same keypair.
#[test]
fn agent_id_deterministic_from_keypair() {
    let dir = tempfile::tempdir().unwrap();
    let paths = axon::config::AxonPaths::from_root(std::path::PathBuf::from(dir.path()));
    let id1 = axon::identity::Identity::load_or_generate(&paths).unwrap();
    let id2 = axon::identity::Identity::load_or_generate(&paths).unwrap();
    assert_eq!(id1.agent_id(), id2.agent_id());
}

// =========================================================================
// §5 IPC — protocol shapes
// =========================================================================

/// `spec/IPC.md` command schema: `{"cmd":"peers"}` is valid.
#[test]
fn ipc_peers_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_str(r#"{"cmd":"peers"}"#).unwrap();
    assert!(matches!(cmd, axon::ipc::IpcCommand::Peers { .. }));
}

/// `spec/IPC.md` command schema: `{"cmd":"status"}` is valid.
#[test]
fn ipc_status_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
    assert!(matches!(cmd, axon::ipc::IpcCommand::Status { .. }));
}

/// `spec/IPC.md` command schema: `{"cmd":"add_peer","pubkey":"...","addr":"host:port"}` is valid.
#[test]
fn ipc_add_peer_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "add_peer",
        "pubkey": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        "addr": "127.0.0.1:7100"
    }))
    .unwrap();
    assert!(matches!(cmd, axon::ipc::IpcCommand::AddPeer { .. }));
}

/// `spec/IPC.md` send command includes to, kind, and payload.
#[test]
fn ipc_send_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "send",
        "to": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "kind": "request",
        "payload": {"question": "test?"}
    }))
    .unwrap();
    match cmd {
        axon::ipc::IpcCommand::Send { to, kind, .. } => {
            assert_eq!(to, "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            assert_eq!(kind, axon::ipc::IpcSendKind::Request);
        }
        _ => panic!("expected Send"),
    }
}

/// `spec/IPC.md` peers response uses canonical `agent_id` (not legacy `id`).
#[test]
fn ipc_peers_response_uses_agent_id_field() {
    let reply = axon::ipc::DaemonReply::Peers {
        ok: true,
        peers: vec![axon::ipc::PeerSummary {
            agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            addr: "127.0.0.1:7100".to_string(),
            status: "connected".to_string(),
            rtt_ms: Some(1.23),
            source: "static".to_string(),
        }],
        req_id: None,
    };

    let j: Value = serde_json::to_value(&reply).unwrap();
    assert!(j["peers"][0].get("agent_id").is_some());
    assert!(j["peers"][0].get("id").is_none());
}

/// `spec/IPC.md` error response has ok=false and an error code string.
#[test]
fn ipc_error_response_shape() {
    let reply = axon::ipc::DaemonReply::Error {
        ok: false,
        error: axon::ipc::IpcErrorCode::PeerNotFound,
        message: axon::ipc::IpcErrorCode::PeerNotFound.message(),
        req_id: None,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["ok"], false);
    assert_eq!(j["error"], "peer_not_found");
}

/// `spec/IPC.md` error code table includes all daemon-emitted IPC error codes.
#[test]
fn ipc_error_codes_match_spec_table() {
    let expected = vec![
        "invalid_command".to_string(),
        "command_too_large".to_string(),
        "peer_not_found".to_string(),
        "self_send".to_string(),
        "peer_unreachable".to_string(),
        "timeout".to_string(),
        "internal_error".to_string(),
    ];
    let actual: Vec<String> = vec![
        axon::ipc::IpcErrorCode::InvalidCommand,
        axon::ipc::IpcErrorCode::CommandTooLarge,
        axon::ipc::IpcErrorCode::PeerNotFound,
        axon::ipc::IpcErrorCode::SelfSend,
        axon::ipc::IpcErrorCode::PeerUnreachable,
        axon::ipc::IpcErrorCode::Timeout,
        axon::ipc::IpcErrorCode::InternalError,
    ]
    .into_iter()
    .map(|code| code.to_string())
    .collect();

    assert_eq!(actual, expected);
}

/// `spec/IPC.md` inbound events include `event`, `from`, and `envelope`.
#[test]
fn ipc_inbound_shape() {
    let envelope = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"topic":"t","data":{}}),
    );
    let reply = axon::ipc::DaemonReply::InboundEvent {
        event: "inbound",
        from: agent_a().to_string(),
        envelope,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["event"], "inbound");
    assert!(j["envelope"]["kind"].is_string());
}

/// `spec/IPC.md` pair_request events include event, agent_id, and pubkey.
#[test]
fn ipc_pair_request_shape() {
    let reply = axon::ipc::DaemonReply::PairRequestEvent {
        event: "pair_request",
        agent_id: agent_a().to_string(),
        pubkey: "Zm9v".to_string(),
        addr: Some("127.0.0.1:7100".to_string()),
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["event"], "pair_request");
    assert_eq!(j["agent_id"], agent_a().to_string());
    assert_eq!(j["pubkey"], "Zm9v");
    assert_eq!(j["addr"], "127.0.0.1:7100");
}

// =========================================================================
// Config — static peers per spec §2
// =========================================================================

/// `spec/SPEC.md` static peer config includes agent_id, addr (ip or hostname), pubkey.
#[tokio::test]
async fn config_static_peers_match_spec() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    std::fs::write(
        &path,
        r#"
port: 7100
peers:
  - agent_id: "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    addr: "100.64.0.5:7100"
    pubkey: "cHVia2V5MQ=="
  - agent_id: "ed25519.b1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    addr: "localhost:7101"
    pubkey: "cHVia2V5Mg=="
"#,
    )
    .unwrap();
    let config = axon::config::Config::load(&path).await.unwrap();
    assert_eq!(config.peers.len(), 2);
    assert_eq!(
        config.peers[0].agent_id,
        "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    );
    assert_eq!(config.peers[0].addr.to_string(), "100.64.0.5:7100");
    assert_eq!(config.peers[0].pubkey, "cHVia2V5MQ==");
    assert_eq!(
        config.peers[1].agent_id,
        "ed25519.b1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    );
    assert_eq!(config.peers[1].addr.port(), 7101);
    assert!(config.peers[1].addr.ip().is_loopback());
    assert_eq!(config.peers[1].pubkey, "cHVia2V5Mg==");
}
