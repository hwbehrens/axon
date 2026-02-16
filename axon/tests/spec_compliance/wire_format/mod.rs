use super::*;

mod ipc_v2_shapes;
mod violations;

// =========================================================================
// §4 Wire format — FIN-delimited framing
// =========================================================================

/// spec.md §3: wire format is raw JSON bytes (FIN-delimited, no length prefix).
#[test]
fn wire_format_is_raw_json() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let encoded = encode(&env).unwrap();

    let decoded: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded["kind"], "request");
}

/// spec.md §3: max message size is 64KB.
#[test]
fn max_message_size_is_64kb() {
    assert_eq!(MAX_MESSAGE_SIZE, 65536);
}

/// spec.md §3: messages > 64KB are rejected.
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

/// spec.md §1: Agent ID = "ed25519." + first 16 bytes of SHA-256(public key),
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

/// spec.md §1: agent ID is deterministic from the same keypair.
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

/// spec.md §5: `{"cmd":"peers"}` is a valid IPC command.
#[test]
fn ipc_peers_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_str(r#"{"cmd":"peers"}"#).unwrap();
    assert!(matches!(cmd, axon::ipc::IpcCommand::Peers { .. }));
}

/// spec.md §5: `{"cmd":"status"}` is a valid IPC command.
#[test]
fn ipc_status_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
    assert!(matches!(cmd, axon::ipc::IpcCommand::Status { .. }));
}

/// spec.md §5: send command includes to, kind, and payload.
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
            assert_eq!(kind, MessageKind::Request);
        }
        _ => panic!("expected Send"),
    }
}

/// spec.md §5: daemon error response has ok=false and error string.
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

/// spec.md §5: inbound messages forwarded with envelope.
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

// =========================================================================
// Config — static peers per spec §2
// =========================================================================

/// spec.md §2: static peers from config.toml with agent_id, addr, pubkey.
#[test]
fn config_static_peers_match_spec() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
port = 7100

[[peers]]
agent_id = "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
addr = "100.64.0.5:7100"
pubkey = "cHVia2V5MQ=="
"#,
    )
    .unwrap();
    let config = axon::config::Config::load(&path).unwrap();
    assert_eq!(config.peers.len(), 1);
    assert_eq!(
        config.peers[0].agent_id,
        "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    );
    assert_eq!(config.peers[0].addr.to_string(), "100.64.0.5:7100");
    assert_eq!(config.peers[0].pubkey, "cHVia2V5MQ==");
}
