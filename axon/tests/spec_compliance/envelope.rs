use super::*;

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
        "from": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "to": "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "ts": 1771108000000,
        "kind": "notify",
        "payload": {"topic": "meta.status", "data": {}},
        "extra_field": "should be ignored",
        "another_unknown": 42
    }"#;
    let decoded: Envelope = serde_json::from_str(raw).unwrap();
    assert_eq!(decoded.kind, MessageKind::Notify);
}
