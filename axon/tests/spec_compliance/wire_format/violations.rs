use super::*;

// =========================================================================
// §10 Protocol Violation — Message Kind Classification
// =========================================================================

/// spec.md §10: Unknown kind deserialized from wire uses #[serde(other)].
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

/// spec.md §10: Request kinds (expects_response=true) on uni stream should be dropped.
#[test]
fn request_kinds_classified_for_uni_drop() {
    assert!(
        MessageKind::Request.expects_response(),
        "Request should be classified as expects_response, would be dropped on uni"
    );
}

/// spec.md §10: Fire-and-forget kinds should NOT expect a response.
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
        MessageKind::Request,
        json!({"question": big}),
    );
    assert!(encode(&env).is_err());
}

/// spec.md §10: Duplicate message IDs should be unique (UUID v4 guarantee).
#[test]
fn message_ids_are_unique() {
    let env1 = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let env2 = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    assert_ne!(env1.id, env2.id, "UUID v4 should generate unique IDs");
}

/// default_error_response returns Error kind for unhandled requests.
#[test]
fn default_error_response_returns_error_kind() {
    let req = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Request,
        json!({}),
    );
    let resp =
        axon::transport::default_error_response(&req, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(resp.kind, MessageKind::Error);
    let payload = resp.payload_value().unwrap();
    assert_eq!(payload["code"], "unhandled");
}
