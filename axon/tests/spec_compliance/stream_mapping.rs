use super::*;

// =========================================================================
// §Stream Mapping — message kind → stream type classification
// =========================================================================

/// Stream Mapping: Request expects a response (bidi), Message does not (uni).
#[test]
fn stream_type_classification_per_spec() {
    // Bidirectional: Request
    assert!(
        MessageKind::Request.expects_response(),
        "Request should be bidirectional (expects_response)"
    );

    // Unidirectional / no response expected: Message, Response, Error
    assert!(
        !MessageKind::Message.expects_response(),
        "Message should be unidirectional"
    );
    assert!(
        !MessageKind::Response.expects_response(),
        "Response should not expect a response"
    );
    assert!(
        !MessageKind::Error.expects_response(),
        "Error should not expect a response"
    );
}

/// Response and Error are classified as responses.
#[test]
fn response_kinds_classified() {
    assert!(MessageKind::Response.is_response());
    assert!(MessageKind::Error.is_response());
    assert!(!MessageKind::Request.is_response());
    assert!(!MessageKind::Message.is_response());
}

// =========================================================================
// Deserialization from spec example JSON
// =========================================================================

/// Verify that a request envelope can be deserialized.
#[test]
fn deserialize_request_example() {
    let j = json!({
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "from": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "kind": "request",
        "ref": null,
        "payload": {
            "question": "What events are on the family calendar this week?"
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Request);
    assert!(env.ref_id.is_none());
}

/// Verify that a message (fire-and-forget) envelope can be deserialized.
#[test]
fn deserialize_message_example() {
    let j = json!({
        "id": "550e8400-e29b-41d4-a716-446655440002",
        "from": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "kind": "message",
        "ref": null,
        "payload": {
            "topic": "user.location",
            "data": {"status": "heading out", "eta_back": "2h"}
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Message);
}

/// Verify that an error envelope can be deserialized.
#[test]
fn deserialize_error_example() {
    let j = json!({
        "id": "550e8400-e29b-41d4-a716-446655440003",
        "from": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "to": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "kind": "error",
        "ref": "550e8400-e29b-41d4-a716-446655440000",
        "payload": {
            "code": "unknown_domain",
            "message": "I don't have access to work calendars.",
            "retryable": false
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Error);
    assert!(env.ref_id.is_some());
}

/// Unknown kind on wire deserializes to Unknown via serde(other).
#[test]
fn deserialize_unknown_kind() {
    let j = json!({
        "id": "550e8400-e29b-41d4-a716-446655440010",
        "kind": "future_kind_v99",
        "payload": {}
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Unknown);
}
