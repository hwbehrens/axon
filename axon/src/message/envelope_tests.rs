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
        MessageKind::Request,
        json!({"question": "hello"}),
    );
    let encoded = serde_json::to_string(&envelope).expect("serialize");
    let decoded: Envelope = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded.kind, MessageKind::Request);
    assert_eq!(decoded.payload_value().unwrap()["question"], json!("hello"));
}

#[test]
fn response_links_request_id() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let resp = Envelope::response_to(
        &req,
        agent_b(),
        MessageKind::Response,
        json!({"result": "ok"}),
    );
    assert_eq!(resp.ref_id, Some(req.id));
    assert_eq!(resp.to, req.from);
}

#[test]
fn envelope_new_sets_defaults() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    assert!(env.ref_id.is_none());
    assert!(env.from.is_some());
    assert!(env.to.is_some());
}

#[test]
fn validation_accepts_valid_envelope() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Message, json!({}));
    assert!(env.validate().is_ok());
}

#[test]
fn validation_rejects_nil_uuid() {
    let mut env = Envelope::new(agent_a(), agent_b(), MessageKind::Message, json!({}));
    env.id = uuid::Uuid::nil();
    assert!(env.validate().is_err());
}

#[test]
fn unknown_envelope_fields_are_ignored() {
    let raw = r#"{
            "id":"6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "kind":"message",
            "payload":{},
            "from":"ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "to":"ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "extra":"ignored"
        }"#;
    let decoded: Envelope = serde_json::from_str(raw).expect("deserialize");
    assert_eq!(decoded.kind, MessageKind::Message);
}

#[test]
fn ref_field_serializes_as_ref_not_ref_id() {
    let env = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let v = serde_json::to_value(&env).unwrap();
    // ref_id is None, so "ref" should not be present (skip_serializing_if)
    assert!(v.get("ref").is_none());
    assert!(v.get("ref_id").is_none());
}

#[test]
fn ref_field_present_when_set() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let resp = Envelope::response_to(
        &req,
        agent_b(),
        MessageKind::Response,
        json!({"result": "ok"}),
    );
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["ref"].as_str().unwrap(), req.id.to_string());
}

#[test]
fn from_and_to_are_optional() {
    let raw = r#"{
            "id":"6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "kind":"message",
            "payload":{}
        }"#;
    let decoded: Envelope = serde_json::from_str(raw).expect("deserialize");
    assert_eq!(decoded.from, None);
    assert_eq!(decoded.to, None);
    assert!(decoded.validate().is_ok());
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

proptest! {
    #[test]
    fn validation_accepts_any_non_nil_uuid(
        a in any::<u128>().prop_filter("non-nil", |v| *v != 0),
    ) {
        let mut env = Envelope::new(agent_a(), agent_b(), MessageKind::Message, json!({}));
        env.id = uuid::Uuid::from_u128(a);
        prop_assert!(env.validate().is_ok());
    }

    #[test]
    fn response_always_links_request(
        from_hex in "[0-9a-f]{32}",
        to_hex in "[0-9a-f]{32}",
    ) {
        let from_id = format!("ed25519.{from_hex}");
        let to_id = format!("ed25519.{to_hex}");
        let req = Envelope::new(from_id, to_id.clone(), MessageKind::Request, json!({"q":"?"}));
        let resp = Envelope::response_to(&req, to_id, MessageKind::Response, json!({}));
        prop_assert_eq!(resp.ref_id, Some(req.id));
        prop_assert_eq!(resp.to, req.from);
    }
}
