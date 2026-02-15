use super::*;

#[test]
fn expects_response_mapping_matches_spec() {
    assert!(MessageKind::Hello.expects_response());
    assert!(MessageKind::Ping.expects_response());
    assert!(MessageKind::Query.expects_response());
    assert!(MessageKind::Delegate.expects_response());
    assert!(MessageKind::Cancel.expects_response());
    assert!(MessageKind::Discover.expects_response());
    assert!(!MessageKind::Notify.expects_response());
    assert!(!MessageKind::Result.expects_response());
    assert!(!MessageKind::Pong.expects_response());
    assert!(!MessageKind::Response.expects_response());
    assert!(!MessageKind::Ack.expects_response());
    assert!(!MessageKind::Capabilities.expects_response());
    assert!(!MessageKind::Error.expects_response());
    assert!(!MessageKind::Unknown.expects_response());
}

#[test]
fn is_response_mapping() {
    assert!(MessageKind::Pong.is_response());
    assert!(MessageKind::Response.is_response());
    assert!(MessageKind::Ack.is_response());
    assert!(MessageKind::Capabilities.is_response());
    assert!(MessageKind::Error.is_response());
    assert!(!MessageKind::Query.is_response());
    assert!(!MessageKind::Notify.is_response());
    assert!(!MessageKind::Unknown.is_response());
}

#[test]
fn is_required_mapping() {
    assert!(MessageKind::Hello.is_required());
    assert!(MessageKind::Ping.is_required());
    assert!(MessageKind::Pong.is_required());
    assert!(MessageKind::Query.is_required());
    assert!(MessageKind::Response.is_required());
    assert!(MessageKind::Notify.is_required());
    assert!(MessageKind::Error.is_required());
    assert!(!MessageKind::Delegate.is_required());
    assert!(!MessageKind::Cancel.is_required());
    assert!(!MessageKind::Discover.is_required());
    assert!(!MessageKind::Unknown.is_required());
}

#[test]
fn message_kind_display() {
    assert_eq!(MessageKind::Hello.to_string(), "hello");
    assert_eq!(MessageKind::Query.to_string(), "query");
    assert_eq!(MessageKind::Capabilities.to_string(), "capabilities");
    assert_eq!(MessageKind::Unknown.to_string(), "unknown");
}

#[test]
fn unknown_kind_deserializes_from_unrecognized_string() {
    let kind: MessageKind = serde_json::from_str(r#""foo_bar_baz""#).unwrap();
    assert_eq!(kind, MessageKind::Unknown);

    let kind: MessageKind = serde_json::from_str(r#""stream""#).unwrap();
    assert_eq!(kind, MessageKind::Unknown);
}

#[test]
fn hello_features_includes_all_optional_kinds() {
    let f = hello_features();
    assert!(f.contains(&"delegate".to_string()));
    assert!(f.contains(&"cancel".to_string()));
    assert!(f.contains(&"discover".to_string()));
    assert!(f.contains(&"capabilities".to_string()));
    assert!(f.contains(&"ack".to_string()));
    assert!(f.contains(&"result".to_string()));
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

const ALL_KINDS: &[MessageKind] = &[
    MessageKind::Hello,
    MessageKind::Ping,
    MessageKind::Pong,
    MessageKind::Query,
    MessageKind::Response,
    MessageKind::Delegate,
    MessageKind::Ack,
    MessageKind::Result,
    MessageKind::Notify,
    MessageKind::Cancel,
    MessageKind::Discover,
    MessageKind::Capabilities,
    MessageKind::Error,
    MessageKind::Unknown,
];

proptest! {
    #[test]
    fn expects_response_xor_is_response_except_hello_error(
        kind_idx in 0..ALL_KINDS.len(),
    ) {
        let kind = ALL_KINDS[kind_idx];
        if kind != MessageKind::Hello
            && kind != MessageKind::Error
            && kind != MessageKind::Notify
            && kind != MessageKind::Result
            && kind != MessageKind::Unknown
        {
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
