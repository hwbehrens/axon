use super::*;
use serde_json::json;

fn agent_a() -> AgentId {
    AgentId::parse("ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4").unwrap()
}

fn agent_b() -> AgentId {
    AgentId::parse("ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3").unwrap()
}

#[test]
fn malformed_envelope_reason_is_bounded() {
    // serde_json embeds an unexpected string in its error text; an attacker
    // could otherwise push the whole message into a log line. SPEC.md §9
    // requires the drop to be logged, so the logged reason must be capped.
    let huge = "x".repeat(4096);
    let err = serde_json::from_str::<u64>(&format!("\"{huge}\"")).expect_err("not a u64");
    let text = err.to_string();
    let reason = super::streams::bounded_malformed_reason(&err);
    assert!(
        reason.chars().count() <= 257,
        "logged reason must be bounded, got {} chars",
        reason.chars().count()
    );
    if text.chars().count() > 257 {
        assert!(reason.ends_with('…'), "truncation must be marked");
    } else {
        assert_eq!(reason, text, "short reasons pass through unchanged");
    }
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
fn budget_exhaustion_classifies_phases_by_payload_delivery() {
    // PRE-delivery phases (dial lock, resolution, handshake, stream open,
    // and the fire-and-forget frame check) can only fail before a payload
    // byte exists: they stay non-ambiguous so an at-most-once retry remains
    // safe and the daemon can keep the transport's phase detail without
    // claiming a response was awaited.
    let past = Instant::now() - Duration::from_secs(1);
    for phase in [
        "per-peer dial lock",
        "QUIC handshake",
        "bidi stream open",
        "uni frame write",
    ] {
        let error = remaining_budget(past, phase).expect_err("exhausted deadline");
        assert!(error.timed_out, "{phase} must stay a timeout: {error}");
        assert!(
            !error.ambiguous,
            "{phase} happens before any payload byte was written"
        );
        assert!(
            error.inner.to_string().contains(phase),
            "the phase detail must survive for {phase}: {error}"
        );
    }

    // POST-delivery phases (request frame write, response read) may already
    // have delivered the request: classify them ambiguous so the daemon
    // keeps the delivery-status warning instead of claiming the peer was
    // never reached.
    for phase in ["request frame write", "response read"] {
        let error = remaining_budget_after_write(past, phase).expect_err("exhausted deadline");
        assert!(
            error.timed_out && error.ambiguous,
            "{phase} may already be delivered, so it must be ambiguous: {error}"
        );
        assert!(
            error.inner.to_string().contains(phase),
            "the phase detail must survive for {phase}: {error}"
        );
    }
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
