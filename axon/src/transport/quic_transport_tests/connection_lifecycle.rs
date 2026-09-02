use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::fixtures::{make_transport_pair, make_transport_trio};
use crate::message::{AgentId, Envelope, MessageKind};

#[tokio::test]
async fn simultaneous_cross_dial_converges_to_one_connection() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let message_a = Envelope::new(
        agent_a.clone(),
        agent_b.clone(),
        MessageKind::Message,
        json!({"from": "a"}),
    );
    let message_b = Envelope::new(
        agent_b.clone(),
        agent_a.clone(),
        MessageKind::Message,
        json!({"from": "b"}),
    );

    let (sent_a, sent_b) = tokio::join!(
        pair.transport_a.send_to(
            &pair.directory_a,
            &agent_b,
            message_a,
            Duration::from_secs(5),
        ),
        pair.transport_b.send_to(
            &pair.directory_b,
            &agent_a,
            message_b,
            Duration::from_secs(5),
        ),
    );
    sent_a.expect("A to B");
    sent_b.expect("B to A");

    let deadline = Instant::now() + Duration::from_secs(5);
    while (pair.transport_a.connected_count().await != 1
        || pair.transport_b.connected_count().await != 1)
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(pair.transport_a.connected_count().await, 1);
    assert_eq!(pair.transport_b.connected_count().await, 1);
}

#[tokio::test]
async fn explicit_refresh_advances_slot_and_reconnects() {
    let pair = make_transport_pair().await;
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let first = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Message,
        json!({"attempt": 1}),
    );
    pair.transport_a
        .send_to(&pair.directory_a, &agent_b, first, Duration::from_secs(5))
        .await
        .expect("first send");
    pair.transport_a.close_peer(&agent_b, b"test refresh").await;
    assert!(!pair.transport_a.has_connection(&agent_b).await);

    let second = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Message,
        json!({"attempt": 2}),
    );
    pair.transport_a
        .send_to(&pair.directory_a, &agent_b, second, Duration::from_secs(5))
        .await
        .expect("send after refresh");
    assert!(pair.transport_a.has_connection(&agent_b).await);
}

// ---------------------------------------------------------------------------
// Peer-scoped enrollment epochs
//
// Regression: revoking peer B used to bump a GLOBAL epoch, rejecting an
// otherwise valid in-flight handshake for unrelated peer A. The epoch is now
// per-peer: only the revoked peer's stale attempts are refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoking_one_peer_does_not_reject_an_unrelated_peers_handshake() {
    let trio = make_transport_trio().await;

    // Simulate an in-flight handshake for C on A's side: capture C's
    // enrollment epoch exactly as a dial would, BEFORE any revocation.
    let captured_c = trio.transport_a.peer_enrollment_epoch(&trio.agent_c);

    // Revoke B.
    trio.transport_a
        .close_peer(&trio.agent_b, b"revoke b")
        .await;

    // B's pre-revocation attempt (captured epoch 0) is rejected...
    assert!(!trio.transport_a.admission_gate(trio.agent_b.clone(), 0)());
    // ...while C's equally-old in-flight attempt remains fully admissible:
    // its epoch did not move and it is still enrolled.
    assert_eq!(
        trio.transport_a.peer_enrollment_epoch(&trio.agent_c),
        captured_c
    );
    assert!(trio
        .transport_a
        .admission_gate(trio.agent_c.clone(), captured_c)());

    // End to end: A↔C exchange works after B's revocation.
    let message = Envelope::new(
        AgentId::parse(trio.id_a.agent_id()).unwrap(),
        trio.agent_c.clone(),
        MessageKind::Message,
        json!({"hello": "c"}),
    );
    trio.transport_a
        .send_to(
            &trio.directory_a,
            &trio.agent_c,
            message,
            Duration::from_secs(5),
        )
        .await
        .expect("send to unrelated peer must succeed after revoking B");
}

#[tokio::test]
async fn poisoned_epoch_lock_fails_admission_closed() {
    let trio = make_transport_trio().await;
    let manager = &trio.transport_a;

    // Poison the epoch lock: a holder panics while holding it.
    let epochs = Arc::clone(&manager.enrollment_epochs);
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _guard = epochs.lock().expect("lock before poisoning");
        panic!("intentional: poison the enrollment epoch lock");
    }));

    // Admission fails CLOSED for every captured epoch while poisoned.
    assert!(!manager.admission_gate(trio.agent_b.clone(), 0)());
    let captured = manager.peer_enrollment_epoch(&trio.agent_c);
    assert!(!manager.admission_gate(trio.agent_c.clone(), captured)());

    // Captures default safely instead of panicking.
    assert_eq!(manager.peer_enrollment_epoch(&trio.agent_c), 0);
    assert!(manager.capture_enrollment_epochs().is_empty());

    // close_peer (epoch bump + registry teardown) must not panic while
    // poisoned; the bump is skipped with a warning and the gate keeps
    // failing closed.
    manager.close_peer(&trio.agent_c, b"poison test").await;
    assert!(!manager.admission_gate(trio.agent_c.clone(), 0)());
}

#[tokio::test]
async fn stale_epoch_attempt_is_rejected_while_fresh_capture_is_admissible() {
    let trio = make_transport_trio().await;
    trio.transport_a
        .close_peer(&trio.agent_b, b"revoke b")
        .await;
    let bumped = trio.transport_a.peer_enrollment_epoch(&trio.agent_b);

    // Stale pre-revocation attempt: rejected by the epoch mismatch even
    // though the directory still lists B as enrolled.
    assert!(!trio.transport_a.admission_gate(trio.agent_b.clone(), 0)());
    // An attempt that started after the revocation committed captures the
    // new epoch. Trust removal itself is the directory's authority
    // (`remove_peer`); while B remains enrolled there, such an attempt is
    // admissible — but it can never be confused with the pre-revocation
    // generation (no ABA reuse of epoch 0).
    assert!(trio
        .transport_a
        .admission_gate(trio.agent_b.clone(), bumped)());
}
