use super::*;

// =========================================================================
// §4 Envelope — JSON shape
// =========================================================================

/// Envelope: every message has id, kind, payload at the top level.
/// `from` and `to` are optional (populated by daemon, not on wire).
#[test]
fn envelope_contains_all_required_fields() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let j = to_json(&env);

    assert!(j.get("id").is_some(), "missing 'id'");
    assert!(j.get("from").is_some(), "missing 'from'");
    assert!(j.get("to").is_some(), "missing 'to'");
    assert!(j.get("kind").is_some(), "missing 'kind'");
    assert!(j.get("payload").is_some(), "missing 'payload'");
}

/// Envelope: `ref` is null for initiating messages.
#[test]
fn initiating_message_has_null_ref() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let j = to_json(&env);
    // ref should either be null or absent (skip_serializing_if)
    let r = j.get("ref");
    assert!(
        r.is_none() || r.unwrap().is_null(),
        "initiating message ref should be null or absent"
    );
}

/// Envelope: response `ref` contains the request message ID.
#[test]
fn response_message_has_ref_set() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let resp = Envelope::response_to(&req, agent_b(), MessageKind::Response, json!({}));
    let j = to_json(&resp);
    assert_eq!(j["ref"].as_str().unwrap(), req.id.to_string());
}

/// Envelope: `id` is UUID v4.
#[test]
fn id_is_uuid_v4() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    assert_eq!(env.id.get_version_num(), 4, "id is not UUID v4");
}

/// `spec/WIRE_FORMAT.md`: unknown fields must be ignored (forward compatibility).
#[test]
fn unknown_envelope_fields_ignored() {
    let raw = r#"{
        "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
        "from": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "to": "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "kind": "message",
        "payload": {"topic": "meta.status", "data": {}},
        "extra_field": "should be ignored",
        "another_unknown": 42
    }"#;
    let decoded: Envelope = serde_json::from_str(raw).unwrap();
    assert_eq!(decoded.kind, MessageKind::Message);
}
