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

/// `spec/IPC.md` command schema: candidate enrollment is explicit by Agent ID.
#[test]
fn ipc_add_peer_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "add_peer",
        "agent_id": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
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
            public_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            trust: "enrolled",
            locators: vec!["127.0.0.1:7100".to_string()],
            status: "connected",
            rtt_ms: Some(1.23),
            display_name: None,
            services: None,
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
        message: axon::ipc::IpcErrorCode::PeerNotFound.message().to_string(),
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
        "invalid_command",
        "command_too_large",
        "peer_not_found",
        "peer_not_observed",
        "peer_conflict",
        "self_send",
        "peer_unreachable",
        "timeout",
        "handler_busy",
        "not_handler",
        "request_not_found",
        "send_capacity_exceeded",
        "message_too_large",
        "internal_error",
    ];
    let actual: Vec<String> = vec![
        axon::ipc::IpcErrorCode::InvalidCommand,
        axon::ipc::IpcErrorCode::CommandTooLarge,
        axon::ipc::IpcErrorCode::PeerNotFound,
        axon::ipc::IpcErrorCode::PeerNotObserved,
        axon::ipc::IpcErrorCode::PeerConflict,
        axon::ipc::IpcErrorCode::SelfSend,
        axon::ipc::IpcErrorCode::PeerUnreachable,
        axon::ipc::IpcErrorCode::Timeout,
        axon::ipc::IpcErrorCode::HandlerBusy,
        axon::ipc::IpcErrorCode::NotHandler,
        axon::ipc::IpcErrorCode::RequestNotFound,
        axon::ipc::IpcErrorCode::SendCapacityExceeded,
        axon::ipc::IpcErrorCode::MessageTooLarge,
        axon::ipc::IpcErrorCode::InternalError,
    ]
    .into_iter()
    .map(|code| {
        let value = serde_json::to_value(code).unwrap();
        value.as_str().unwrap().to_owned()
    })
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

/// `spec/IPC.md` candidate events include identity, locators, and source.
#[test]
fn ipc_peer_candidate_shape() {
    let reply = axon::ipc::DaemonReply::PeerCandidateEvent {
        event: "peer_candidate",
        agent_id: agent_a().to_string(),
        public_key: "Zm9v".to_string(),
        locators: vec!["127.0.0.1:7100".to_string()],
        source: "mdns",
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["event"], "peer_candidate");
    assert_eq!(j["agent_id"], agent_a().to_string());
    assert_eq!(j["public_key"], "Zm9v");
    assert_eq!(j["locators"][0], "127.0.0.1:7100");
}

// =========================================================================
// Config — local settings only
// =========================================================================

/// `spec/SPEC.md` rejects legacy peer authority in config.yaml.
#[tokio::test]
async fn config_rejects_legacy_static_peers() {
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
"#,
    )
    .unwrap();
    assert!(axon::config::Config::load(&path).await.is_err());
}

// =========================================================================
// §6.5 Capability manifests — describe kind and manifest payload conformance
// =========================================================================

/// spec/MESSAGE_TYPES.md + spec/WIRE_FORMAT.md §6.5: `describe` is a
/// bidirectional kind; its `response` payload is a capability manifest whose
/// encoded form carries only id/kind/ref/payload on the wire.
#[test]
fn describe_exchange_conformance() {
    use axon::manifest::{MAX_MANIFEST_BYTES, Manifest};

    // Request: kind "describe", payload ignored (spec: SHOULD be {}).
    let request = Envelope::new(agent_a(), agent_b(), MessageKind::Describe, json!({}));
    let request_bytes = request.wire_encode().unwrap();
    let decoded_request: Value = serde_json::from_slice(&request_bytes).unwrap();
    assert_eq!(decoded_request["kind"], "describe");
    assert!(decoded_request.get("from").is_none());
    assert!(decoded_request.get("to").is_none());

    // Response: manifest payload with the normative schema fields. Built via
    // the wire path (JSON parse) — the only external construction route.
    let manifest: Manifest = serde_json::from_value(json!({
        "name": "forge",
        "version": "0.9.0",
        "services": [{
            "id": "cargo_test",
            "description": "Run cargo test on a workspace.",
            "example_request": {"workspace": "/srv/axon"},
            "timeout_hint_secs": 900,
            "concurrency": 2,
            "errors": ["build_failed"]
        }]
    }))
    .unwrap();
    assert!(
        manifest.encoded_size().unwrap() <= MAX_MANIFEST_BYTES,
        "schema-valid manifests must fit the §6.5 encoded bound"
    );

    let response = Envelope::response_to(
        &request,
        agent_b(),
        MessageKind::Response,
        manifest.to_payload_value().unwrap(),
    );
    let response_bytes = response.wire_encode().unwrap();
    let decoded: Value = serde_json::from_slice(&response_bytes).unwrap();
    assert_eq!(decoded["kind"], "response");
    assert_eq!(decoded["ref"], json!(request.id.to_string()));
    assert_eq!(decoded["payload"]["name"], "forge");
    assert_eq!(decoded["payload"]["services"][0]["id"], "cargo_test");
    assert_eq!(
        decoded["payload"]["services"][0]["timeout_hint_secs"],
        json!(900)
    );

    // Round-trip: the wire payload parses back into an equal Manifest
    // (unknown fields ignored, per §6.5 forward compatibility).
    let mut with_future_field = decoded["payload"].clone();
    with_future_field["future_field"] = json!(true);
    let reparsed: Manifest = serde_json::from_value(with_future_field).unwrap();
    assert_eq!(reparsed, manifest);
}

/// spec/MESSAGE_TYPES.md forward-compatibility note: a peer that predates
/// `describe` sees a plain unknown kind string on the wire and replies
/// `unsupported_kind` naming it. Simulate that legacy receiver with a
/// string-typed kind field.
#[test]
fn describe_is_a_lossless_string_for_legacy_receivers() {
    #[derive(serde::Deserialize)]
    struct LegacyEnvelope {
        kind: String,
    }

    let request = Envelope::new(agent_a(), agent_b(), MessageKind::Describe, json!({}));
    let bytes = encode(&request).unwrap();
    let legacy: LegacyEnvelope = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(legacy.kind, "describe");
}
