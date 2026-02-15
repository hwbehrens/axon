//! Wire protocol compliance tests.
//!
//! Each test verifies a specific requirement from `spec/spec.md` or
//! `spec/message-types.md` (v2). Tests are grouped by spec section.

use axon::message::*;
use serde_json::{json, Value};

// =========================================================================
// Helpers
// =========================================================================

fn agent_a() -> String {
    "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8".to_string()
}

fn agent_b() -> String {
    "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

fn to_json(env: &Envelope) -> Value {
    serde_json::to_value(env).unwrap()
}

// =========================================================================
// §4 Envelope — JSON shape
// =========================================================================

/// message-types.md §Envelope: every message has v, id, from, to, ts, kind,
/// ref, payload at the top level.
#[test]
fn envelope_contains_all_required_fields() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let j = to_json(&env);

    assert!(j.get("v").is_some(), "missing 'v'");
    assert!(j.get("id").is_some(), "missing 'id'");
    assert!(j.get("from").is_some(), "missing 'from'");
    assert!(j.get("to").is_some(), "missing 'to'");
    assert!(j.get("ts").is_some(), "missing 'ts'");
    assert!(j.get("kind").is_some(), "missing 'kind'");
    assert!(j.get("payload").is_some(), "missing 'payload'");
}

/// message-types.md §Envelope: `ref` is null for initiating messages.
#[test]
fn initiating_message_has_null_ref() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let j = to_json(&env);
    // ref should either be null or absent (skip_serializing_if)
    let r = j.get("ref");
    assert!(
        r.is_none() || r.unwrap().is_null(),
        "initiating message ref should be null or absent"
    );
}

/// message-types.md §Envelope: response `ref` contains the request message ID.
#[test]
fn response_message_has_ref_set() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let resp = Envelope::response_to(&req, agent_b(), MessageKind::Pong, json!({}));
    let j = to_json(&resp);
    assert_eq!(j["ref"].as_str().unwrap(), req.id.to_string());
}

/// message-types.md §Envelope: `ts` is unix milliseconds.
#[test]
fn ts_is_unix_milliseconds() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    // Must be in milliseconds: > Jan 1 2025 00:00:00 UTC
    assert!(
        env.ts > 1_735_689_600_000,
        "ts should be unix milliseconds, got {}",
        env.ts
    );
}

/// message-types.md §Envelope: `id` is UUID v4.
#[test]
fn id_is_uuid_v4() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    assert_eq!(env.id.get_version_num(), 4, "id is not UUID v4");
}

/// message-types.md §Envelope: `v` is protocol version 1.
#[test]
fn v_is_protocol_version() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Query, json!({}));
    assert_eq!(env.v, PROTOCOL_VERSION);
    assert_eq!(env.v, 1);
}

/// spec.md §4: unknown fields MUST be ignored (forward compatibility).
#[test]
fn unknown_envelope_fields_ignored() {
    let raw = r#"{
        "v": 1,
        "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
        "from": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "to": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "ts": 1771108000000,
        "kind": "notify",
        "payload": {"topic": "meta.status", "data": {}},
        "extra_field": "should be ignored",
        "another_unknown": 42
    }"#;
    let decoded: Envelope = serde_json::from_str(raw).unwrap();
    assert_eq!(decoded.kind, MessageKind::Notify);
}

// =========================================================================
// §Payload Schemas — round-trips matching spec examples
// =========================================================================

/// Spec example hello initiating payload.
#[test]
fn hello_initiating_matches_spec_shape() {
    let payload = HelloPayload {
        protocol_versions: vec![1],
        selected_version: None,
        agent_name: Some("TestAgent".to_string()),
        features: vec!["delegate".to_string(), "discover".to_string()],
    };
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Hello,
        serde_json::to_value(&payload).unwrap(),
    );
    let j = to_json(&env);
    assert_eq!(j["kind"], "hello");
    let p = &j["payload"];
    assert_eq!(p["protocol_versions"], json!([1]));
    assert_eq!(p["agent_name"], "TestAgent");
    assert_eq!(p["features"], json!(["delegate", "discover"]));
    // Initiating hello should NOT have selected_version
    assert!(
        p.get("selected_version").is_none() || p["selected_version"].is_null(),
        "initiating hello should not have selected_version"
    );
}

/// Spec example hello response payload.
#[test]
fn hello_response_matches_spec_shape() {
    let payload = HelloPayload {
        protocol_versions: vec![1],
        selected_version: Some(1),
        agent_name: None,
        features: vec![
            "delegate".to_string(),
            "discover".to_string(),
            "cancel".to_string(),
        ],
    };
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Hello, json!({}));
    let resp = Envelope::response_to(
        &req,
        agent_b(),
        MessageKind::Hello,
        serde_json::to_value(&payload).unwrap(),
    );
    let j = to_json(&resp);
    assert_eq!(j["kind"], "hello");
    assert_eq!(j["ref"].as_str().unwrap(), req.id.to_string());
    let p = &j["payload"];
    assert_eq!(p["selected_version"], 1);
    assert_eq!(p["features"], json!(["delegate", "discover", "cancel"]));
}

/// message-types.md §ping: empty payload.
#[test]
fn ping_payload_is_empty_object() {
    let p = PingPayload {};
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v, json!({}));
}

/// message-types.md §pong: status, uptime_secs, active_tasks, agent_name.
#[test]
fn pong_payload_matches_spec() {
    let p = PongPayload {
        status: PeerStatus::Idle,
        uptime_secs: 3600,
        active_tasks: 2,
        agent_name: Some("MyAgent".to_string()),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["status"], "idle");
    assert_eq!(v["uptime_secs"], 3600);
    assert_eq!(v["active_tasks"], 2);
    assert_eq!(v["agent_name"], "MyAgent");
}

/// message-types.md §query: question, domain, max_tokens, deadline_ms.
#[test]
fn query_payload_matches_spec() {
    let p = QueryPayload {
        question: "What events are on the family calendar this week?".to_string(),
        domain: Some("family.calendar".to_string()),
        max_tokens: Some(200),
        deadline_ms: Some(30000),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["question"], "What events are on the family calendar this week?");
    assert_eq!(v["domain"], "family.calendar");
    assert_eq!(v["max_tokens"], 200);
    assert_eq!(v["deadline_ms"], 30000);
}

/// message-types.md §response: data, summary, tokens_used, truncated.
#[test]
fn response_payload_matches_spec() {
    let p = ResponsePayload {
        data: json!({"events": [{"name": "swim practice"}]}),
        summary: "Three swim practices this week: Mon/Wed/Fri 4-5pm".to_string(),
        tokens_used: Some(47),
        truncated: Some(false),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["summary"], "Three swim practices this week: Mon/Wed/Fri 4-5pm");
    assert_eq!(v["tokens_used"], 47);
    assert_eq!(v["truncated"], false);
}

/// message-types.md §delegate: task, context, priority, report_back, deadline_ms.
#[test]
fn delegate_payload_matches_spec() {
    let p = DelegatePayload {
        task: "Send a message to the family group chat about dinner plans".to_string(),
        context: Some(json!({"dinner_time": "7:00 PM", "location": "home"})),
        priority: Priority::Normal,
        report_back: true,
        deadline_ms: Some(60000),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["task"], "Send a message to the family group chat about dinner plans");
    assert_eq!(v["priority"], "normal");
    assert_eq!(v["report_back"], true);
    assert_eq!(v["deadline_ms"], 60000);
    assert_eq!(v["context"]["dinner_time"], "7:00 PM");
}

/// message-types.md §delegate: priority defaults to "normal", report_back to true.
#[test]
fn delegate_defaults_per_spec() {
    let d: DelegatePayload = serde_json::from_value(json!({"task": "do something"})).unwrap();
    assert_eq!(d.priority, Priority::Normal);
    assert!(d.report_back);
    assert!(d.context.is_none());
    assert!(d.deadline_ms.is_none());
}

/// message-types.md §ack: accepted, estimated_ms.
#[test]
fn ack_payload_matches_spec() {
    let p = AckPayload {
        accepted: true,
        estimated_ms: Some(5000),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["accepted"], true);
    assert_eq!(v["estimated_ms"], 5000);
}

/// message-types.md §result: status, outcome, data, error.
#[test]
fn result_payload_matches_spec() {
    let p = ResultPayload {
        status: TaskStatus::Completed,
        outcome: "Message sent. Got a thumbs-up reaction.".to_string(),
        data: Some(json!({"reaction": "👍"})),
        error: None,
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["status"], "completed");
    assert_eq!(v["outcome"], "Message sent. Got a thumbs-up reaction.");
    assert!(v.get("error").is_none() || v["error"].is_null());
}

/// message-types.md §notify: topic, data, importance.
#[test]
fn notify_payload_matches_spec() {
    let p = NotifyPayload {
        topic: "user.location".to_string(),
        data: json!({"status": "heading out", "eta_back": "2h"}),
        importance: Importance::Low,
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["topic"], "user.location");
    assert_eq!(v["data"]["eta_back"], "2h");
    assert_eq!(v["importance"], "low");
}

/// message-types.md §notify: importance defaults to low.
#[test]
fn notify_importance_defaults_to_low() {
    let n: NotifyPayload = serde_json::from_value(json!({"topic": "t", "data": {}})).unwrap();
    assert_eq!(n.importance, Importance::Low);
}

/// message-types.md §cancel: reason field.
#[test]
fn cancel_payload_matches_spec() {
    let p = CancelPayload {
        reason: Some("Plans changed, no longer needed".to_string()),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["reason"], "Plans changed, no longer needed");
}

/// message-types.md §discover: empty payload.
#[test]
fn discover_payload_is_empty() {
    let p = DiscoverPayload {};
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v, json!({}));
}

/// message-types.md §capabilities: all fields present.
#[test]
fn capabilities_payload_matches_spec() {
    let p = CapabilitiesPayload {
        agent_name: Some("Family Assistant".to_string()),
        domains: vec!["family".to_string(), "calendar".to_string()],
        channels: vec!["imessage".to_string()],
        tools: vec!["web_search".to_string(), "calendar_cli".to_string()],
        max_concurrent_tasks: Some(4),
        model: Some("gemini-3-pro".to_string()),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["agent_name"], "Family Assistant");
    assert_eq!(v["domains"], json!(["family", "calendar"]));
    assert_eq!(v["channels"], json!(["imessage"]));
    assert_eq!(v["max_concurrent_tasks"], 4);
    assert_eq!(v["model"], "gemini-3-pro");
}

/// message-types.md §error: code, message, retryable.
#[test]
fn error_payload_matches_spec() {
    let p = ErrorPayload {
        code: ErrorCode::UnknownDomain,
        message: "I don't have access to work calendars. Try querying the work agent.".to_string(),
        retryable: false,
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["code"], "unknown_domain");
    assert!(v["message"].as_str().unwrap().contains("work calendars"));
    assert_eq!(v["retryable"], false);
}

// =========================================================================
// §Stream Mapping — message kind → stream type classification
// =========================================================================

/// message-types.md §Stream Mapping: kinds that expect responses use bidi,
/// fire-and-forget kinds use unidirectional.
#[test]
fn stream_type_classification_per_spec() {
    // Bidirectional: hello, ping, query, delegate, cancel, discover
    let bidi_kinds = [
        MessageKind::Hello,
        MessageKind::Ping,
        MessageKind::Query,
        MessageKind::Delegate,
        MessageKind::Cancel,
        MessageKind::Discover,
    ];
    for kind in &bidi_kinds {
        assert!(
            kind.expects_response(),
            "{kind} should be bidirectional (expects_response)"
        );
    }

    // Unidirectional: notify, result
    assert!(
        !MessageKind::Notify.expects_response(),
        "notify should be unidirectional"
    );
    assert!(
        !MessageKind::Result.expects_response(),
        "result should be unidirectional"
    );
}

/// message-types.md §Core Types: required kinds are hello, ping, pong, query,
/// response, notify, error.
#[test]
fn required_kinds_per_spec() {
    let required = [
        MessageKind::Hello,
        MessageKind::Ping,
        MessageKind::Pong,
        MessageKind::Query,
        MessageKind::Response,
        MessageKind::Notify,
        MessageKind::Error,
    ];
    for kind in &required {
        assert!(kind.is_required(), "{kind} should be required");
    }
}

/// message-types.md §Core Types: optional kinds are delegate, ack, result,
/// cancel, discover, capabilities.
#[test]
fn optional_kinds_per_spec() {
    let optional = [
        MessageKind::Delegate,
        MessageKind::Ack,
        MessageKind::Result,
        MessageKind::Cancel,
        MessageKind::Discover,
        MessageKind::Capabilities,
    ];
    for kind in &optional {
        assert!(!kind.is_required(), "{kind} should be optional");
    }
}

/// hello_features() should advertise all optional kinds.
#[test]
fn hello_features_advertises_all_optional() {
    let f = hello_features();
    assert!(f.contains(&"delegate".to_string()));
    assert!(f.contains(&"ack".to_string()));
    assert!(f.contains(&"result".to_string()));
    assert!(f.contains(&"cancel".to_string()));
    assert!(f.contains(&"discover".to_string()));
    assert!(f.contains(&"capabilities".to_string()));
}

// =========================================================================
// Deserialization from spec example JSON
// =========================================================================

/// Verify that the exact JSON shapes from spec/message-types.md can be
/// deserialized.
#[test]
fn deserialize_spec_query_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "from": "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "query",
        "ref": null,
        "payload": {
            "question": "What events are on the family calendar this week?",
            "domain": "family.calendar",
            "max_tokens": 200,
            "deadline_ms": 30000
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Query);
    assert!(env.ref_id.is_none());
}

#[test]
fn deserialize_spec_delegate_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440001",
        "from": "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "delegate",
        "ref": null,
        "payload": {
            "task": "Send a message to the family group chat about dinner plans",
            "context": {"dinner_time": "7:00 PM", "location": "home"},
            "priority": "normal",
            "report_back": true,
            "deadline_ms": 60000
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Delegate);
}

#[test]
fn deserialize_spec_notify_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440002",
        "from": "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "notify",
        "ref": null,
        "payload": {
            "topic": "user.location",
            "data": {"status": "heading out", "eta_back": "2h"},
            "importance": "low"
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Notify);
}

#[test]
fn deserialize_spec_error_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440003",
        "from": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "to": "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "ts": 1771108000001_u64,
        "kind": "error",
        "ref": "550e8400-e29b-41d4-a716-446655440000",
        "payload": {
            "code": "unknown_domain",
            "message": "I don't have access to work calendars. Try querying the work agent.",
            "retryable": false
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Error);
    assert!(env.ref_id.is_some());
}

#[test]
fn deserialize_spec_hello_initiating() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440010",
        "from": "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "hello",
        "ref": null,
        "payload": {
            "protocol_versions": [1],
            "agent_name": "Family Assistant",
            "features": ["delegate", "discover"]
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Hello);
    let h: HelloPayload = serde_json::from_value(env.payload).unwrap();
    assert_eq!(h.protocol_versions, vec![1]);
    assert_eq!(h.agent_name, Some("Family Assistant".to_string()));
    assert!(h.selected_version.is_none());
}

#[test]
fn deserialize_spec_hello_response() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440011",
        "from": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "to": "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "ts": 1771108000001_u64,
        "kind": "hello",
        "ref": "550e8400-e29b-41d4-a716-446655440010",
        "payload": {
            "protocol_versions": [1],
            "selected_version": 1,
            "agent_name": "Work Assistant",
            "features": ["delegate", "discover", "cancel"]
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Hello);
    let h: HelloPayload = serde_json::from_value(env.payload).unwrap();
    assert_eq!(h.selected_version, Some(1));
    assert_eq!(h.features, vec!["delegate", "discover", "cancel"]);
    assert!(env.ref_id.is_some());
}

#[test]
fn deserialize_spec_capabilities_response() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440020",
        "from": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "to": "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "ts": 1771108000001_u64,
        "kind": "capabilities",
        "ref": "550e8400-e29b-41d4-a716-446655440019",
        "payload": {
            "agent_name": "Family Assistant",
            "domains": ["family", "calendar", "groceries", "school"],
            "channels": ["imessage", "apple-reminders"],
            "tools": ["web_search", "calendar_cli"],
            "max_concurrent_tasks": 4,
            "model": "gemini-3-pro"
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Capabilities);
    let c: CapabilitiesPayload = serde_json::from_value(env.payload).unwrap();
    assert_eq!(c.domains, vec!["family", "calendar", "groceries", "school"]);
    assert_eq!(c.model, Some("gemini-3-pro".to_string()));
}

// =========================================================================
// §4 Wire format — length-prefix framing
// =========================================================================

/// spec.md §3: each message has [u32 length prefix, big-endian] [message bytes].
#[test]
fn wire_format_length_prefix_is_big_endian_u32() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let encoded = encode(&env).unwrap();

    let len = u32::from_be_bytes(encoded[..4].try_into().unwrap());
    assert_eq!(len as usize, encoded.len() - 4);
    assert!(len > 0);
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
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Query, json!({"question": big}));
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
    let decoded = decode(&encoded[4..]).unwrap();
    assert_eq!(env.id, decoded.id);
    assert_eq!(env.from, decoded.from);
    assert_eq!(env.to, decoded.to);
    assert_eq!(env.kind, decoded.kind);
    assert_eq!(env.payload, decoded.payload);
    assert_eq!(env.v, decoded.v);
    assert_eq!(env.ts, decoded.ts);
}

// =========================================================================
// §1 Identity — agent ID derivation
// =========================================================================

/// spec.md §1: Agent ID = first 16 bytes of SHA-256(public key), hex-encoded
/// (32 chars).
#[test]
fn agent_id_is_32_hex_chars() {
    let paths =
        axon::config::AxonPaths::from_root(std::path::PathBuf::from(tempfile::tempdir().unwrap().path()));
    let identity = axon::identity::Identity::load_or_generate(&paths).unwrap();
    let id = identity.agent_id();
    assert_eq!(id.len(), 32);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
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
        "to": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "kind": "query",
        "payload": {"question": "test?"}
    }))
    .unwrap();
    match cmd {
        axon::ipc::IpcCommand::Send { to, kind, .. } => {
            assert_eq!(to, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
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
    let envelope = Envelope::new(agent_a(), agent_b(), MessageKind::Notify, json!({"topic":"t","data":{}}));
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
agent_id = "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
addr = "100.64.0.5:7100"
pubkey = "cHVia2V5MQ=="
"#,
    )
    .unwrap();
    let config = axon::config::Config::load(&path).unwrap();
    assert_eq!(config.peers.len(), 1);
    assert_eq!(config.peers[0].agent_id, "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8");
    assert_eq!(config.peers[0].addr.to_string(), "100.64.0.5:7100");
    assert_eq!(config.peers[0].pubkey, "cHVia2V5MQ==");
}
