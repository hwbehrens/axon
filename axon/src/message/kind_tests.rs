use super::*;

#[test]
fn expects_response_mapping() {
    assert!(MessageKind::Request.expects_response());
    assert!(!MessageKind::Response.expects_response());
    assert!(!MessageKind::Message.expects_response());
    assert!(!MessageKind::Error.expects_response());
    assert!(!MessageKind::Unknown.expects_response());
}

#[test]
fn is_response_mapping() {
    assert!(MessageKind::Response.is_response());
    assert!(MessageKind::Error.is_response());
    assert!(!MessageKind::Request.is_response());
    assert!(!MessageKind::Message.is_response());
    assert!(!MessageKind::Unknown.is_response());
}

#[test]
fn message_kind_display() {
    assert_eq!(MessageKind::Request.to_string(), "request");
    assert_eq!(MessageKind::Response.to_string(), "response");
    assert_eq!(MessageKind::Message.to_string(), "message");
    assert_eq!(MessageKind::Error.to_string(), "error");
    assert_eq!(MessageKind::Unknown.to_string(), "unknown");
}

#[test]
fn serde_roundtrip() {
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

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

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
