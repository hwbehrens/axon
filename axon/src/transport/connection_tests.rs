use super::*;
use serde_json::json;

fn agent_a() -> String {
    "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string()
}

fn agent_b() -> String {
    "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

#[test]
fn default_error_response_contract() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let resp = default_error_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Error);
    assert_eq!(resp.ref_id, Some(req.id));
    assert_eq!(resp.from.as_deref(), Some(agent_b().as_str()));
    assert_eq!(resp.to.as_deref(), Some(agent_a().as_str()));
    let payload = resp.payload_value().unwrap();
    assert_eq!(
        payload.get("code").and_then(|v| v.as_str()),
        Some("unhandled")
    );
    assert!(payload.get("message").and_then(|v| v.as_str()).is_some());
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

fn arb_kind() -> impl Strategy<Value = MessageKind> {
    prop_oneof![
        Just(MessageKind::Request),
        Just(MessageKind::Response),
        Just(MessageKind::Message),
        Just(MessageKind::Error),
        Just(MessageKind::Unknown),
    ]
}

proptest! {
    #[test]
    fn default_error_response_always_returns_error(kind in arb_kind()) {
        let req = Envelope::new(agent_a(), agent_b(), kind, json!({}));
        let resp = default_error_response(&req, &agent_b());
        prop_assert_eq!(resp.kind, MessageKind::Error);
        prop_assert_eq!(resp.ref_id, Some(req.id));
        let payload = resp.payload_value().unwrap();
        prop_assert!(payload.get("code").and_then(|v| v.as_str()).is_some());
        prop_assert!(payload.get("message").and_then(|v| v.as_str()).is_some());
    }
}
