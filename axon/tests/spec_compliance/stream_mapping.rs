use super::*;

// =========================================================================
// §Stream Mapping — message kind → stream type classification
// =========================================================================

/// message-types.md §Stream Mapping: kinds that expect responses use bidi,
/// fire-and-forget kinds use unidirectional.
#[test]
fn stream_type_classification_per_spec() {
    // Bidirectional: hello, ping, query, delegate, cancel, discover
    let bidi_kinds = [
        MessageKind::Hello,
        MessageKind::Ping,
        MessageKind::Query,
        MessageKind::Delegate,
        MessageKind::Cancel,
        MessageKind::Discover,
    ];
    for kind in &bidi_kinds {
        assert!(
            kind.expects_response(),
            "{kind} should be bidirectional (expects_response)"
        );
    }

    // Unidirectional: notify, result
    assert!(
        !MessageKind::Notify.expects_response(),
        "notify should be unidirectional"
    );
    assert!(
        !MessageKind::Result.expects_response(),
        "result should be unidirectional"
    );
}

/// message-types.md §Core Types: required kinds are hello, ping, pong, query,
/// response, notify, error.
#[test]
fn required_kinds_per_spec() {
    let required = [
        MessageKind::Hello,
        MessageKind::Ping,
        MessageKind::Pong,
        MessageKind::Query,
        MessageKind::Response,
        MessageKind::Notify,
        MessageKind::Error,
    ];
    for kind in &required {
        assert!(kind.is_required(), "{kind} should be required");
    }
}

/// message-types.md §Core Types: optional kinds are delegate, ack, result,
/// cancel, discover, capabilities.
#[test]
fn optional_kinds_per_spec() {
    let optional = [
        MessageKind::Delegate,
        MessageKind::Ack,
        MessageKind::Result,
        MessageKind::Cancel,
        MessageKind::Discover,
        MessageKind::Capabilities,
    ];
    for kind in &optional {
        assert!(!kind.is_required(), "{kind} should be optional");
    }
}

/// hello_features() should advertise all optional kinds.
#[test]
fn hello_features_advertises_all_optional() {
    let f = hello_features();
    assert!(f.contains(&"delegate".to_string()));
    assert!(f.contains(&"ack".to_string()));
    assert!(f.contains(&"result".to_string()));
    assert!(f.contains(&"cancel".to_string()));
    assert!(f.contains(&"discover".to_string()));
    assert!(f.contains(&"capabilities".to_string()));
}

// =========================================================================
// Deserialization from spec example JSON
// =========================================================================

/// Verify that the exact JSON shapes from spec/MESSAGE_TYPES.md can be
/// deserialized.
#[test]
fn deserialize_spec_query_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "from": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "query",
        "ref": null,
        "payload": {
            "question": "What events are on the family calendar this week?",
            "domain": "family.calendar",
            "max_tokens": 200,
            "deadline_ms": 30000
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Query);
    assert!(env.ref_id.is_none());
}

#[test]
fn deserialize_spec_delegate_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440001",
        "from": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "delegate",
        "ref": null,
        "payload": {
            "task": "Send a message to the family group chat about dinner plans",
            "context": {"dinner_time": "7:00 PM", "location": "home"},
            "priority": "normal",
            "report_back": true,
            "deadline_ms": 60000
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Delegate);
}

#[test]
fn deserialize_spec_notify_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440002",
        "from": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "notify",
        "ref": null,
        "payload": {
            "topic": "user.location",
            "data": {"status": "heading out", "eta_back": "2h"},
            "importance": "low"
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Notify);
}

#[test]
fn deserialize_spec_error_example() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440003",
        "from": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "to": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "ts": 1771108000001_u64,
        "kind": "error",
        "ref": "550e8400-e29b-41d4-a716-446655440000",
        "payload": {
            "code": "unknown_domain",
            "message": "I don't have access to work calendars. Try querying the work agent.",
            "retryable": false
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Error);
    assert!(env.ref_id.is_some());
}

#[test]
fn deserialize_spec_hello_initiating() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440010",
        "from": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "to": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "ts": 1771108000000_u64,
        "kind": "hello",
        "ref": null,
        "payload": {
            "protocol_versions": [1],
            "agent_name": "Family Assistant",
            "features": ["delegate", "discover"]
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Hello);
    let h: HelloPayload = serde_json::from_str(env.payload.get()).unwrap();
    assert_eq!(h.protocol_versions, vec![1]);
    assert_eq!(h.agent_name, Some("Family Assistant".to_string()));
    assert!(h.selected_version.is_none());
}

#[test]
fn deserialize_spec_hello_response() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440011",
        "from": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "to": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "ts": 1771108000001_u64,
        "kind": "hello",
        "ref": "550e8400-e29b-41d4-a716-446655440010",
        "payload": {
            "protocol_versions": [1],
            "selected_version": 1,
            "agent_name": "Work Assistant",
            "features": ["delegate", "discover", "cancel"]
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Hello);
    let h: HelloPayload = serde_json::from_str(env.payload.get()).unwrap();
    assert_eq!(h.selected_version, Some(1));
    assert_eq!(h.features, vec!["delegate", "discover", "cancel"]);
    assert!(env.ref_id.is_some());
}

#[test]
fn deserialize_spec_capabilities_response() {
    let j = json!({
        "v": 1,
        "id": "550e8400-e29b-41d4-a716-446655440020",
        "from": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "to": "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8",
        "ts": 1771108000001_u64,
        "kind": "capabilities",
        "ref": "550e8400-e29b-41d4-a716-446655440019",
        "payload": {
            "agent_name": "Family Assistant",
            "domains": ["family", "calendar", "groceries", "school"],
            "channels": ["imessage", "apple-reminders"],
            "tools": ["web_search", "calendar_cli"],
            "max_concurrent_tasks": 4,
            "model": "gemini-3-pro"
        }
    });
    let env: Envelope = serde_json::from_value(j).unwrap();
    assert_eq!(env.kind, MessageKind::Capabilities);
    let c: CapabilitiesPayload = serde_json::from_str(env.payload.get()).unwrap();
    assert_eq!(c.domains, vec!["family", "calendar", "groceries", "school"]);
    assert_eq!(c.model, Some("gemini-3-pro".to_string()));
}
