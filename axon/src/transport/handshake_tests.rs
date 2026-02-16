use super::*;
use serde_json::json;

fn agent_a() -> String {
    "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string()
}

fn agent_b() -> String {
    "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

#[test]
fn auto_response_ping() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
    let resp = auto_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Pong);
    assert_eq!(resp.ref_id, Some(req.id));
}

#[test]
fn auto_response_discover() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Discover, json!({}));
    let resp = auto_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Capabilities);
}

#[test]
fn auto_response_query() {
    let req = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Query,
        json!({"question": "test?"}),
    );
    let resp = auto_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Response);
}

#[test]
fn auto_response_delegate() {
    let req = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Delegate,
        json!({"task": "do something"}),
    );
    let resp = auto_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Ack);
}

#[test]
fn auto_response_cancel() {
    let req = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Cancel,
        json!({"reason": "changed mind"}),
    );
    let resp = auto_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Ack);
}

#[test]
fn auto_response_unknown_kind() {
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Pong, json!({}));
    let resp = auto_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Error);
    assert_eq!(
        resp.payload_value()
            .unwrap()
            .get("code")
            .and_then(|v| v.as_str()),
        Some("unknown_kind")
    );
}

#[test]
fn auto_response_hello_returns_error() {
    let req = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Hello,
        json!({"protocol_versions": [1], "features": ["delegate"]}),
    );
    let resp = auto_response(&req, &agent_b());
    assert_eq!(resp.kind, MessageKind::Error);
    assert_eq!(
        resp.payload_value()
            .unwrap()
            .get("code")
            .and_then(|v| v.as_str()),
        Some("unknown_kind")
    );
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

const REQUEST_RESPONSE_MAP: &[(MessageKind, MessageKind)] = &[
    (MessageKind::Ping, MessageKind::Pong),
    (MessageKind::Query, MessageKind::Response),
    (MessageKind::Delegate, MessageKind::Ack),
    (MessageKind::Cancel, MessageKind::Ack),
    (MessageKind::Discover, MessageKind::Capabilities),
];

proptest! {
    #[test]
    fn auto_response_kind_correctness(idx in 0..REQUEST_RESPONSE_MAP.len()) {
        let (req_kind, expected_resp_kind) = REQUEST_RESPONSE_MAP[idx];
        let req = Envelope::new(agent_a(), agent_b(), req_kind, json!({}));
        let resp = auto_response(&req, &agent_b());
        prop_assert_eq!(resp.kind, expected_resp_kind,
            "auto_response({:?}) should be {:?}, got {:?}", req_kind, expected_resp_kind, resp.kind);
    }
}
