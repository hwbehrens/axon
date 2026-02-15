use super::*;

// =========================================================================
// §4 Wire format — FIN-delimited framing
// =========================================================================

/// spec.md §3: wire format is raw JSON bytes (FIN-delimited, no length prefix).
#[test]
fn wire_format_is_raw_json() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let encoded = encode(&env).unwrap();

    let decoded: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded["kind"], "ping");
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
        MessageKind::Query,
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
        MessageKind::Query,
        json!({"question": "what is 2+2?", "domain": "math"}),
    );
    let encoded = encode(&env).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(env.id, decoded.id);
    assert_eq!(env.from, decoded.from);
    assert_eq!(env.to, decoded.to);
    assert_eq!(env.kind, decoded.kind);
    assert_eq!(env.payload.get(), decoded.payload.get());
    assert_eq!(env.v, decoded.v);
    assert_eq!(env.ts, decoded.ts);
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
    assert!(matches!(cmd, axon::ipc::IpcCommand::Peers));
}

/// spec.md §5: `{"cmd":"status"}` is a valid IPC command.
#[test]
fn ipc_status_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
    assert!(matches!(cmd, axon::ipc::IpcCommand::Status));
}

/// spec.md §5: send command includes to, kind, and payload.
#[test]
fn ipc_send_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "send",
        "to": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "kind": "query",
        "payload": {"question": "test?"}
    }))
    .unwrap();
    match cmd {
        axon::ipc::IpcCommand::Send { to, kind, .. } => {
            assert_eq!(to, "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            assert_eq!(kind, MessageKind::Query);
        }
        _ => panic!("expected Send"),
    }
}

/// spec.md §5: daemon error response has ok=false and error string.
#[test]
fn ipc_error_response_shape() {
    let reply = axon::ipc::DaemonReply::Error {
        ok: false,
        error: "peer not found: deadbeef".to_string(),
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["ok"], false);
    assert!(j["error"].as_str().unwrap().contains("peer not found"));
}

/// spec.md §5: inbound messages forwarded with envelope.
#[test]
fn ipc_inbound_shape() {
    let envelope = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Notify,
        json!({"topic":"t","data":{}}),
    );
    let reply = axon::ipc::DaemonReply::Inbound {
        inbound: true,
        envelope,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["inbound"], true);
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

// =========================================================================
// §10 Protocol Violation — Message Kind Classification
// =========================================================================

/// spec.md §10: Unknown kind deserialized from wire uses #[serde(other)].
#[test]
fn unknown_kind_from_wire() {
    let raw = r#"{
        "v": 1,
        "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
        "from": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "to": "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "ts": 1771108000000,
        "kind": "future_kind_v99",
        "payload": {}
    }"#;
    let env: Envelope = serde_json::from_str(raw).unwrap();
    assert_eq!(env.kind, MessageKind::Unknown);
}

/// spec.md §10: Request kinds (expects_response=true) on uni stream should be dropped.
/// Verify classification: these kinds are the ones connection.rs will drop on uni.
#[test]
fn request_kinds_classified_for_uni_drop() {
    let request_kinds = [
        MessageKind::Hello,
        MessageKind::Ping,
        MessageKind::Query,
        MessageKind::Delegate,
        MessageKind::Cancel,
        MessageKind::Discover,
    ];
    for kind in &request_kinds {
        assert!(
            kind.expects_response(),
            "{kind} should be classified as request (expects_response), would be dropped on uni"
        );
    }
}

/// spec.md §10: Fire-and-forget kinds should NOT expect a response.
/// On bidi stream, these are forwarded and the send side is finished.
#[test]
fn fire_and_forget_kinds_classified() {
    let faf_kinds = [
        MessageKind::Notify,
        MessageKind::Result,
        MessageKind::Pong,
        MessageKind::Response,
        MessageKind::Ack,
        MessageKind::Capabilities,
    ];
    for kind in &faf_kinds {
        assert!(
            !kind.expects_response(),
            "{kind} should NOT expect a response (fire-and-forget)"
        );
    }
}

/// spec.md §10: Envelope validation catches invalid agent IDs.
/// Invalid envelopes should be dropped/rejected per violation handling.
#[test]
fn invalid_envelope_detected_for_violation_handling() {
    let invalid = Envelope::new(
        "bad_id".to_string(),
        "also_bad".to_string(),
        MessageKind::Notify,
        json!({"topic": "test", "data": {}}),
    );
    assert!(invalid.validate().is_err());
}

/// spec.md §10: Malformed JSON on any stream should be dropped.
#[test]
fn malformed_json_fails_deserialization() {
    let bad_json = b"this is not json{{{";
    let result = serde_json::from_slice::<Envelope>(bad_json);
    assert!(result.is_err());
}

/// spec.md §10: Oversized messages (>64KB) should be rejected.
#[test]
fn oversized_message_rejected_by_encode() {
    let big = "x".repeat(MAX_MESSAGE_SIZE as usize);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Query,
        json!({"question": big}),
    );
    assert!(encode(&env).is_err());
}

/// spec.md §10: Duplicate message IDs should be unique (UUID v4 guarantee).
#[test]
fn message_ids_are_unique() {
    let env1 = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let env2 = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    assert_ne!(env1.id, env2.id, "UUID v4 should generate unique IDs");
}

/// spec.md §10: Version mismatch in hello triggers error(incompatible_version).
#[test]
fn version_mismatch_produces_incompatible_version_error() {
    let req = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Hello,
        json!({"protocol_versions": [99]}),
    );
    let resp = axon::transport::auto_response(&req, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(resp.kind, MessageKind::Error);
    let payload = resp.payload_value().unwrap();
    assert_eq!(payload["code"], "incompatible_version");
}
