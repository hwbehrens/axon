use super::*;

// =========================================================================
// §10 Protocol Violation — Message Kind Classification
// =========================================================================

/// `spec/WIRE_FORMAT.md` unknown-kind compatibility: deserialization uses `#[serde(other)]`.
#[test]
fn unknown_kind_from_wire() {
    let raw = r#"{
        "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
        "kind": "future_kind_v99",
        "payload": {}
    }"#;
    let env: Envelope = serde_json::from_str(raw).unwrap();
    assert_eq!(env.kind, MessageKind::Unknown);
}

/// `spec/WIRE_FORMAT.md` stream mapping: request kinds on uni stream should be dropped.
#[test]
fn request_kinds_classified_for_uni_drop() {
    assert!(
        MessageKind::Request.expects_response(),
        "Request should be classified as expects_response, would be dropped on uni"
    );
}

/// `spec/WIRE_FORMAT.md` stream mapping: fire-and-forget kinds should not expect a response.
#[test]
fn fire_and_forget_kinds_classified() {
    let faf_kinds = [
        MessageKind::Message,
        MessageKind::Response,
        MessageKind::Error,
    ];
    for kind in &faf_kinds {
        assert!(
            !kind.expects_response(),
            "{kind} should NOT expect a response (fire-and-forget)"
        );
    }
}

/// `spec/WIRE_FORMAT.md` decoding: malformed JSON on any stream should be dropped.
#[test]
fn malformed_json_fails_deserialization() {
    let bad_json = b"this is not json{{{";
    let result = serde_json::from_slice::<Envelope>(bad_json);
    assert!(result.is_err());
}

/// `spec/WIRE_FORMAT.md` limits: oversized messages (>64KB) should be rejected.
#[test]
fn oversized_message_rejected_by_encode() {
    let big = "x".repeat(MAX_MESSAGE_SIZE as usize);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"question": big}),
    );
    assert!(encode(&env).is_err());
}

/// `spec/WIRE_FORMAT.md` envelope ids are UUID v4 and should be unique in practice.
#[test]
fn message_ids_are_unique() {
    let env1 = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let env2 = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    assert_ne!(env1.id, env2.id, "UUID v4 should generate unique IDs");
}
