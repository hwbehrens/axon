use super::*;
use serde_json::json;

fn agent_a() -> AgentId {
    AgentId::parse("ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4").unwrap()
}

fn agent_b() -> AgentId {
    AgentId::parse("ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3").unwrap()
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
        Just(MessageKind::unknown("future_kind")),
    ]
}

proptest! {
    #[test]
    fn default_error_response_always_returns_error(kind in arb_kind()) {
        let req = Envelope::new(agent_a(), agent_b(), kind, json!({}));
        let resp = default_error_response(&req, &agent_b());
        prop_assert_eq!(&resp.kind, &MessageKind::Error);
        prop_assert_eq!(resp.ref_id, Some(req.id));
        let payload = resp.payload_value().unwrap();
        prop_assert!(payload.get("code").and_then(|v| v.as_str()).is_some());
        prop_assert!(payload.get("message").and_then(|v| v.as_str()).is_some());
    }
}

// =========================================================================
// Round-five review regressions: whole-exchange deadline arithmetic.
// =========================================================================

#[test]
fn remaining_budget_is_recomputed_against_the_absolute_deadline() {
    // Whole-exchange budgeting: every phase must recompute what is left of
    // ONE absolute deadline, not receive a fresh full budget.
    let future = Instant::now() + Duration::from_secs(5);
    let budget = remaining_budget(future, "test phase").expect("future deadline has budget");
    assert!(budget <= Duration::from_secs(5));
    assert!(budget > Duration::from_secs(4));

    // An exhausted deadline yields the typed pre-delivery timeout, never a
    // panic or a zero-length wait that could still consume a full phase.
    let past = Instant::now() - Duration::from_secs(1);
    let error = remaining_budget(past, "test phase").expect_err("exhausted deadline");
    assert!(error.timed_out);
    assert!(!error.ambiguous, "nothing was delivered yet");
}

#[test]
fn checked_deadline_overflow_never_panics() {
    // Mirrors send_to's deadline construction: hostile durations must
    // overflow into an error, not an Instant-add panic that leaks caller
    // resources.
    assert!(
        Instant::now()
            .checked_add(Duration::from_secs(u64::MAX))
            .is_none()
    );
}
