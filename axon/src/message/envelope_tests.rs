use super::*;
use serde_json::json;

fn agent_a() -> AgentId {
    AgentId::parse("ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4").unwrap()
}

fn agent_b() -> AgentId {
    AgentId::parse("ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3").unwrap()
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
        let from_id = AgentId::parse(&format!("ed25519.{from_hex}")).unwrap();
        let to_id = AgentId::parse(&format!("ed25519.{to_hex}")).unwrap();
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
    assert_eq!(
        MessageKind::unknown("future_kind").to_string(),
        "future_kind"
    );
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
    assert_eq!(kind, MessageKind::unknown("foo_bar_baz"));

    let kind: MessageKind = serde_json::from_str(r#""stream""#).unwrap();
    assert_eq!(kind, MessageKind::unknown("stream"));
}

fn all_kinds() -> Vec<MessageKind> {
    vec![
        MessageKind::Request,
        MessageKind::Response,
        MessageKind::Message,
        MessageKind::Error,
        MessageKind::unknown("future_kind"),
    ]
}

proptest! {
    #[test]
    fn expects_response_xor_is_response_for_known_kinds(
        kind_idx in 0..5usize,
    ) {
        let kind = all_kinds()[kind_idx].clone();
        // Message and Unknown are neither request nor response
        if kind != MessageKind::Message && !matches!(kind, MessageKind::Unknown(_)) {
            prop_assert_ne!(kind.expects_response(), kind.is_response(),
                "kind {:?} must be exactly one of request or response", kind);
        }
    }

    #[test]
    fn display_roundtrips_through_serde(kind_idx in 0..5usize) {
        let kind = all_kinds()[kind_idx].clone();
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

// =========================================================================
// Mutation-coverage: decode entrypoint boundaries and trait semantics
// =========================================================================

#[test]
fn decode_roundtrips_an_encoded_envelope() {
    let envelope = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"ok": true}),
    );

    let decoded = decode(&encode(&envelope).unwrap()).unwrap();

    assert_eq!(decoded, envelope);
}

#[test]
fn decode_accepts_exactly_max_size_and_rejects_one_byte_more() {
    let env_template = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": ""}),
    );
    let base_len = serde_json::to_vec(&env_template).unwrap().len();
    let target = MAX_MESSAGE_SIZE as usize;
    // 'a' needs no JSON escaping, so one char adds exactly one byte.
    let at_limit = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": "a".repeat(target - base_len)}),
    );
    let bytes = serde_json::to_vec(&at_limit).unwrap();
    assert_eq!(bytes.len(), target, "fixture must sit exactly at the bound");

    decode(&bytes).expect("a wire frame of exactly MAX_MESSAGE_SIZE decodes");

    let over_limit = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": "a".repeat(target - base_len + 1)}),
    );
    let oversized = serde_json::to_vec(&over_limit).unwrap();
    assert_eq!(oversized.len(), target + 1);
    assert!(
        decode(&oversized).is_err(),
        "a wire frame beyond MAX_MESSAGE_SIZE must be rejected"
    );
}

fn owned_of(s: &str) -> String {
    s.to_owned()
}

#[test]
fn agent_id_string_comparisons_are_exact() {
    let agent_id = AgentId::parse("ed25519.0123456789abcdef0123456789abcdef").unwrap();

    // Typed bindings pin each impl: &str, String, and unsized str.
    let borrowed: &str = "ed25519.0123456789abcdef0123456789abcdef";
    let owned: String = borrowed.to_owned();
    assert!(agent_id == *borrowed);
    assert!(agent_id == borrowed, "&str comparison");
    assert!(agent_id == owned, "String comparison");

    let wrong_owned: String = "ed25519.ffffffffffffffffffffffffffffffff".to_owned();
    assert!(
        agent_id != owned_of(&wrong_owned),
        "String comparison is exact"
    );

    assert!(agent_id != *"ed25519.ffffffffffffffffffffffffffffffff");
    assert!(
        agent_id != "ED25519.0123456789ABCDEF0123456789ABCDEF",
        "comparison is against the canonical lowercase form"
    );
    assert_eq!(
        agent_id.as_ref(),
        "ed25519.0123456789abcdef0123456789abcdef",
        "as_ref exposes the canonical form"
    );
}

#[test]
fn agent_id_borrow_supports_str_keyed_lookup() {
    use std::collections::HashMap;

    let agent_id = AgentId::parse("ed25519.0123456789abcdef0123456789abcdef").unwrap();
    let mut registry = HashMap::new();
    registry.insert(agent_id.clone(), 1u8);

    assert!(
        !registry.contains_key(agent_id.as_str().split_at(8).0),
        "partial strings must not match"
    );
    assert_eq!(registry.get(agent_id.as_str()), Some(&1));
}

#[test]
fn envelope_equality_compares_every_field() {
    let mut base = Envelope::new(agent_a(), agent_b(), MessageKind::Message, json!({}));
    base.ref_id = Some(Uuid::new_v4());
    let twin = base.clone();
    assert_eq!(base, twin);

    let with = |mutate: fn(&mut Envelope)| {
        let mut other = base.clone();
        mutate(&mut other);
        other
    };
    assert_ne!(
        base,
        with(|e| e.id = Uuid::new_v4()),
        "id participates in equality"
    );
    assert_ne!(
        base,
        with(|e| e.kind = MessageKind::Error),
        "kind participates in equality"
    );
    assert_ne!(
        base,
        with(|e| e.ref_id = None),
        "ref participates in equality"
    );
    assert_ne!(
        base,
        with(|e| e.payload = serde_json::value::to_raw_value(&json!({"x": 1})).unwrap()),
        "payload participates in equality"
    );
    assert_ne!(
        base,
        with(|e| e.from = None),
        "from participates in equality"
    );
    assert_ne!(
        base,
        with(|e| e.to = Some(AgentId::parse("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap())),
        "to participates in equality"
    );
}

#[test]
fn now_millis_returns_plausible_unix_time() {
    let millis = now_millis();
    assert!(
        millis > 1_700_000_000_000,
        "timestamp {millis} is implausibly old for any current clock"
    );
}

#[test]
fn unidirectional_streams_reject_request_and_response_kinds() {
    // Message, Error, and unknown kinds travel unidirectionally; request and
    // response require a bidirectional stream (spec/MESSAGE_TYPES.md).
    assert!(MessageKind::Message.is_allowed_on_unidirectional());
    assert!(MessageKind::Error.is_allowed_on_unidirectional());
    assert!(MessageKind::unknown("future-kind").is_allowed_on_unidirectional());
    assert!(!MessageKind::Request.is_allowed_on_unidirectional());
    assert!(!MessageKind::Response.is_allowed_on_unidirectional());
}

#[test]
fn agent_id_parse_enforces_prefix_and_hex_shape() {
    let parsed = AgentId::parse("ED25519.AABBCCDDEEFF00112233445566778899").unwrap();
    assert_eq!(
        parsed.as_str(),
        "ed25519.aabbccddeeff00112233445566778899",
        "prefix and hex are canonicalized to lowercase"
    );

    // Wrong prefix is rejected before hex validation.
    assert!(AgentId::parse("rsa.aabbccddeeff00112233445566778899").is_err());
    // Wrong length: both shorter and longer.
    assert!(AgentId::parse("ed25519.aabb").is_err());
    assert!(AgentId::parse("ed25519.aabbccddeeff00112233445566778899aabb").is_err());
    // Right length but non-hex characters — the || in parse must not
    // degrade to &&, or this input would be accepted.
    assert!(AgentId::parse("ed25519.zzbbccddeeff00112233445566778899").is_err());
}

#[test]
fn matches_pubkey_base64_reports_binding_truthfully() {
    let key = STANDARD.encode([7u8; 32]);
    let matching = AgentId::from_pubkey_base64(&key).unwrap();

    assert!(matching.matches_pubkey_base64(&key).unwrap());
    let other_key = STANDARD.encode([8u8; 32]);
    assert!(!matching.matches_pubkey_base64(&other_key).unwrap());
}
