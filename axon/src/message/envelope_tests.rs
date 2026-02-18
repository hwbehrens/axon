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
fn validation_rejects_non_v4_uuid() {
    let mut env = Envelope::new(agent_a(), agent_b(), MessageKind::Message, json!({}));
    env.id = uuid::Uuid::parse_str("f81d4fae-7dec-11d0-a765-00a0c91e6bf6").unwrap();
    assert!(env.validate().is_err());
}

#[test]
fn validation_rejects_non_object_payload() {
    let raw = r#"{
            "id":"6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "kind":"message",
            "payload":[1,2,3]
        }"#;
    let decoded: Envelope = serde_json::from_str(raw).expect("deserialize");
    assert!(decoded.validate().is_err());
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

// =========================================================================
// kind tests
// =========================================================================

#[test]
fn message_kind_display() {
    assert_eq!(MessageKind::Request.to_string(), "request");
    assert_eq!(MessageKind::Response.to_string(), "response");
    assert_eq!(MessageKind::Message.to_string(), "message");
    assert_eq!(MessageKind::Error.to_string(), "error");
    assert_eq!(MessageKind::Unknown.to_string(), "unknown");
}

#[test]
fn kind_serde_roundtrip() {
    for kind in [
        MessageKind::Request,
        MessageKind::Response,
        MessageKind::Message,
        MessageKind::Error,
    ] {
        let json = serde_json::to_string(&kind).unwrap();
        let back: MessageKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }
}

#[test]
fn unknown_kind_deserializes_from_unrecognized_string() {
    let kind: MessageKind = serde_json::from_str(r#""foo_bar_baz""#).unwrap();
    assert_eq!(kind, MessageKind::Unknown);

    let kind: MessageKind = serde_json::from_str(r#""stream""#).unwrap();
    assert_eq!(kind, MessageKind::Unknown);
}

const ALL_KINDS: &[MessageKind] = &[
    MessageKind::Request,
    MessageKind::Response,
    MessageKind::Message,
    MessageKind::Error,
    MessageKind::Unknown,
];

proptest! {
    #[test]
    fn expects_response_xor_is_response_for_known_kinds(
        kind_idx in 0..ALL_KINDS.len(),
    ) {
        let kind = ALL_KINDS[kind_idx];
        // Message and Unknown are neither request nor response
        if kind != MessageKind::Message && kind != MessageKind::Unknown {
            prop_assert_ne!(kind.expects_response(), kind.is_response(),
                "kind {:?} must be exactly one of request or response", kind);
        }
    }

    #[test]
    fn display_roundtrips_through_serde(kind_idx in 0..ALL_KINDS.len()) {
        let kind = ALL_KINDS[kind_idx];
        let serialized = serde_json::to_string(&kind).unwrap();
        let deserialized: MessageKind = serde_json::from_str(&serialized).unwrap();
        prop_assert_eq!(kind, deserialized);
    }
}

// =========================================================================
// wire tests
// =========================================================================

proptest! {
    #[test]
    fn decode_arbitrary_bytes_never_panics(data in proptest::collection::vec(any::<u8>(), 0..128)) {
        let _ = decode(&data);
    }
}

// =========================================================================
// Mutation-coverage: encode accepts exactly MAX_MESSAGE_SIZE
// =========================================================================

#[test]
fn encode_accepts_exactly_max_size() {
    let env_template = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": ""}),
    );
    let base_len = serde_json::to_vec(&env_template).unwrap().len();
    assert!(base_len < MAX_MESSAGE_SIZE as usize);

    // Binary search for the padding length that makes JSON exactly MAX_MESSAGE_SIZE
    let target = MAX_MESSAGE_SIZE as usize;
    // The "data" field value is a string. Increasing by 1 char adds 1 byte to JSON
    // (unless the char needs escaping). Use 'a' which is safe.
    let needed = target - base_len;
    let padding = "a".repeat(needed);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": padding}),
    );
    let json_len = serde_json::to_vec(&env).unwrap().len();
    assert_eq!(
        json_len, target,
        "JSON body should be exactly MAX_MESSAGE_SIZE"
    );
    assert!(
        encode(&env).is_ok(),
        "encode must accept exactly MAX_MESSAGE_SIZE"
    );
}
