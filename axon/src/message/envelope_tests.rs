use super::*;
use serde_json::json;

fn agent_a() -> String {
    "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string()
}

fn agent_b() -> String {
    "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

#[test]
fn envelope_round_trip() {
    let envelope = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Query,
        json!({"question": "hello", "domain": "meta.status"}),
    );
    let encoded = serde_json::to_string(&envelope).expect("serialize");
    let decoded: Envelope = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded.kind, MessageKind::Query);
    assert_eq!(decoded.payload_value().unwrap()["question"], json!("hello"));
}

#[test]
fn response_links_request_id() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let resp = Envelope::response_to(
        &req,
        req.to.clone(),
        MessageKind::Pong,
        json!({"status":"idle", "uptime_secs": 0, "active_tasks": 0}),
    );
    assert_eq!(resp.ref_id, Some(req.id));
    assert_eq!(resp.to, req.from);
}

#[test]
fn envelope_validation_catches_bad_ids() {
    let envelope = Envelope::new(
        "abc".to_string(),
        "def".to_string(),
        MessageKind::Notify,
        json!({"topic":"x", "data": {}}),
    );
    assert!(envelope.validate().is_err());
}

#[test]
fn envelope_validation_catches_non_hex_ids() {
    let envelope = Envelope::new(
        "ed25519.a1b2c3d4e5f6a1b2ZZZZZZZZZZZZZZZZ".to_string(),
        "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string(),
        MessageKind::Notify,
        json!({"topic":"x", "data": {}}),
    );
    assert!(envelope.validate().is_err());
}

#[test]
fn envelope_new_sets_defaults() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    assert_eq!(env.v, 1);
    assert!(env.ref_id.is_none());
    assert!(env.ts > 0);
}

#[test]
fn unknown_envelope_fields_are_ignored() {
    let raw = r#"{
            "v":1,
            "id":"6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "from":"ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "to":"ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "ts":1771108000000,
            "kind":"notify",
            "payload":{"topic":"meta.status","data":{}},
            "extra":"ignored"
        }"#;
    let decoded: Envelope = serde_json::from_str(raw).expect("deserialize");
    assert_eq!(decoded.kind, MessageKind::Notify);
}

#[test]
fn ref_field_serializes_as_ref_not_ref_id() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let v = serde_json::to_value(&env).unwrap();
    assert!(v.get("ref").is_some() || v.get("ref_id").is_none());
    assert!(v["ref"].is_null());
}

#[test]
fn ref_field_present_when_set() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let resp = Envelope::response_to(
        &req,
        agent_b(),
        MessageKind::Pong,
        json!({"status":"idle","uptime_secs":0,"active_tasks":0}),
    );
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["ref"].as_str().unwrap(), req.id.to_string());
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

proptest! {
    #[test]
    fn validation_accepts_valid_hex_ids(
        from_hex in "[0-9a-f]{32}",
        to_hex in "[0-9a-f]{32}",
    ) {
        let from_id = format!("ed25519.{from_hex}");
        let to_id = format!("ed25519.{to_hex}");
        let env = Envelope::new(from_id, to_id, MessageKind::Notify, json!({"topic":"x","data":{}}));
        prop_assert!(env.validate().is_ok());
    }

    #[test]
    fn validation_rejects_wrong_length_ids(
        from_hex in "[0-9a-f]{1,31}",
        to_hex in "[0-9a-f]{1,31}",
    ) {
        let from_id = format!("ed25519.{from_hex}");
        let to_id = format!("ed25519.{to_hex}");
        let env = Envelope::new(from_id, to_id, MessageKind::Notify, json!({"topic":"x","data":{}}));
        prop_assert!(env.validate().is_err());
    }

    #[test]
    fn response_always_links_request(
        from_hex in "[0-9a-f]{32}",
        to_hex in "[0-9a-f]{32}",
    ) {
        let from_id = format!("ed25519.{from_hex}");
        let to_id = format!("ed25519.{to_hex}");
        let req = Envelope::new(from_id, to_id.clone(), MessageKind::Query, json!({"question":"?"}));
        let resp = Envelope::response_to(&req, to_id, MessageKind::Response, json!({}));
        prop_assert_eq!(resp.ref_id, Some(req.id));
        prop_assert_eq!(resp.to, req.from);
    }
}

// =========================================================================
// Mutation-coverage: validate() || chain — each condition independently
// =========================================================================

#[test]
fn validation_rejects_bad_from_with_good_to() {
    let env = Envelope::new(
        "abc".to_string(),
        "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string(),
        MessageKind::Notify,
        json!({"topic":"x", "data": {}}),
    );
    assert!(env.validate().is_err());
}

#[test]
fn validation_rejects_good_from_with_bad_to() {
    let env = Envelope::new(
        "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string(),
        "short".to_string(),
        MessageKind::Notify,
        json!({"topic":"x", "data": {}}),
    );
    assert!(env.validate().is_err());
}

#[test]
fn validation_rejects_non_hex_from_with_good_to() {
    let env = Envelope::new(
        "ed25519.ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
        "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string(),
        MessageKind::Notify,
        json!({"topic":"x", "data": {}}),
    );
    assert!(env.validate().is_err());
}

#[test]
fn validation_rejects_good_from_with_non_hex_to() {
    let env = Envelope::new(
        "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string(),
        "ed25519.ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
        MessageKind::Notify,
        json!({"topic":"x", "data": {}}),
    );
    assert!(env.validate().is_err());
}
