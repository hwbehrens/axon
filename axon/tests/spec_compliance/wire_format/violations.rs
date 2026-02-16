use super::*;

// =========================================================================
// §10 Protocol Violation — Message Kind Classification
// =========================================================================

/// spec.md §10: Unknown kind deserialized from wire uses #[serde(other)].
#[test]
fn unknown_kind_from_wire() {
    let raw = r#"{
        "v": 1,
        "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
        "from": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "to": "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "ts": 1771108000000,
        "kind": "future_kind_v99",
        "payload": {}
    }"#;
    let env: Envelope = serde_json::from_str(raw).unwrap();
    assert_eq!(env.kind, MessageKind::Unknown);
}

/// spec.md §10: Request kinds (expects_response=true) on uni stream should be dropped.
/// Verify classification: these kinds are the ones connection.rs will drop on uni.
#[test]
fn request_kinds_classified_for_uni_drop() {
    let request_kinds = [
        MessageKind::Hello,
        MessageKind::Ping,
        MessageKind::Query,
        MessageKind::Delegate,
        MessageKind::Cancel,
        MessageKind::Discover,
    ];
    for kind in &request_kinds {
        assert!(
            kind.expects_response(),
            "{kind} should be classified as request (expects_response), would be dropped on uni"
        );
    }
}

/// spec.md §10: Fire-and-forget kinds should NOT expect a response.
/// On bidi stream, these are forwarded and the send side is finished.
#[test]
fn fire_and_forget_kinds_classified() {
    let faf_kinds = [
        MessageKind::Notify,
        MessageKind::Result,
        MessageKind::Pong,
        MessageKind::Response,
        MessageKind::Ack,
        MessageKind::Capabilities,
    ];
    for kind in &faf_kinds {
        assert!(
            !kind.expects_response(),
            "{kind} should NOT expect a response (fire-and-forget)"
        );
    }
}

/// spec.md §10: Envelope validation catches invalid agent IDs.
/// Invalid envelopes should be dropped/rejected per violation handling.
#[test]
fn invalid_envelope_detected_for_violation_handling() {
    let invalid = Envelope::new(
        "bad_id".to_string(),
        "also_bad".to_string(),
        MessageKind::Notify,
        json!({"topic": "test", "data": {}}),
    );
    assert!(invalid.validate().is_err());
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
        MessageKind::Query,
        json!({"question": big}),
    );
    assert!(encode(&env).is_err());
}

/// spec.md §10: Duplicate message IDs should be unique (UUID v4 guarantee).
#[test]
fn message_ids_are_unique() {
    let env1 = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let env2 = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    assert_ne!(env1.id, env2.id, "UUID v4 should generate unique IDs");
}

// =========================================================================
// WIRE_FORMAT error codes
// =========================================================================

/// WIRE_FORMAT.md §9.2: invalid_envelope is a valid error code that round-trips
/// through serialization. Added to spec in this PR (was missing).
#[test]
fn invalid_envelope_error_code_in_spec() {
    let error_payload = axon::message::ErrorPayload {
        code: axon::message::ErrorCode::InvalidEnvelope,
        message:
            "envelope validation failed: agent IDs must be in the format ed25519.<32 hex chars>"
                .to_string(),
        retryable: false,
    };
    let json = serde_json::to_value(&error_payload).unwrap();
    assert_eq!(json["code"], "invalid_envelope");
    assert_eq!(json["retryable"], false);

    // Round-trip: deserialize back
    let decoded: axon::message::ErrorPayload = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.code, axon::message::ErrorCode::InvalidEnvelope);
    assert!(!decoded.retryable);
}

/// WIRE_FORMAT.md §9.2: All error codes from the spec must serialize to their
/// expected snake_case string representation.
#[test]
fn all_spec_error_codes_serialize_correctly() {
    use axon::message::ErrorCode;
    let expected = vec![
        (ErrorCode::NotAuthorized, "not_authorized"),
        (ErrorCode::UnknownDomain, "unknown_domain"),
        (ErrorCode::Overloaded, "overloaded"),
        (ErrorCode::Internal, "internal"),
        (ErrorCode::Timeout, "timeout"),
        (ErrorCode::Cancelled, "cancelled"),
        (ErrorCode::IncompatibleVersion, "incompatible_version"),
        (ErrorCode::UnknownKind, "unknown_kind"),
        (ErrorCode::PeerNotFound, "peer_not_found"),
        (ErrorCode::InvalidEnvelope, "invalid_envelope"),
    ];
    for (code, expected_str) in expected {
        let json = serde_json::to_value(&code).unwrap();
        assert_eq!(
            json.as_str().unwrap(),
            expected_str,
            "ErrorCode::{code:?} serializes wrong"
        );
    }
}

/// spec.md §10: Version mismatch in hello triggers error(incompatible_version).
#[test]
fn version_mismatch_produces_incompatible_version_error() {
    let req = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Hello,
        json!({"protocol_versions": [99]}),
    );
    let resp = axon::transport::auto_response(&req, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(resp.kind, MessageKind::Error);
    let payload = resp.payload_value().unwrap();
    assert_eq!(payload["code"], "incompatible_version");
}
