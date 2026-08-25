//! Regression coverage for the concurrency classes behind round-two review:
//!
//! - Revocation must linearize with connection admission (P1): the
//!   enrollment gate is consulted under the registry's admission lock, so a
//!   handshake racing `remove_peer` can never land a live slot.
//! - Retry must retire exactly the failed exchange's connection (P1): a
//!   stale retirement must never destroy the current authoritative slot.
//! - Fire-and-forget retries must preserve at-most-once delivery (P1):
//!   ambiguous failures are not retried for non-request kinds.

use std::time::Duration;

use serde_json::json;

use super::fixtures::make_transport_pair;
use crate::message::{AgentId, Envelope, MessageKind};
use crate::transport::DialPeer;
use crate::transport::connection_registry::{Admission, Direction};

#[test]
fn retry_permitted_allows_only_at_most_once_safe_combinations() {
    let pre_send = crate::transport::connection::SendError {
        inner: anyhow::anyhow!("dial failed"),
        ambiguous: false,
        timed_out: false,
    };
    let ambiguous = crate::transport::connection::SendError {
        inner: anyhow::anyhow!("stream died mid-exchange"),
        ambiguous: true,
        timed_out: false,
    };

    // Requests keep DEC-016's single transport-level retry: reply
    // correlation is specified as at-most-one-reply.
    assert!(crate::transport::connection::retry_permitted(
        &MessageKind::Request,
        &pre_send
    ));
    assert!(crate::transport::connection::retry_permitted(
        &MessageKind::Request,
        &ambiguous
    ));

    // Fire-and-forget kinds may only be retried when provably
    // undelivered; an ambiguous retry could broadcast twice.
    assert!(crate::transport::connection::retry_permitted(
        &MessageKind::Message,
        &pre_send
    ));
    assert!(!crate::transport::connection::retry_permitted(
        &MessageKind::Message,
        &ambiguous
    ));
}

#[tokio::test]
async fn uni_failure_before_any_write_is_classified_pre_send() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr = pair.transport_b.local_addr().unwrap();
    let connection = pair
        .transport_a
        .ensure_connection(&DialPeer {
            agent_id: agent_b.clone(),
            addr,
        })
        .await
        .expect("connect");

    // Tearing down the peer's endpoint closes every connection locally;
    // opening a fresh uni stream then fails before any byte is written.
    pair.transport_b.close_all().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let envelope = Envelope::new(agent_a, agent_b, MessageKind::Message, json!({}));
    let err = crate::transport::connection::send_unidirectional(
        &connection,
        envelope,
        Duration::from_secs(5),
    )
    .await
    .expect_err("send on closed endpoint must fail");
    assert!(
        !err.ambiguous,
        "open-stream failure happens before any payload byte is written"
    );
}

#[tokio::test]
async fn admission_gate_refuses_and_closes_when_not_enrolled() {
    let pair = make_transport_pair().await;
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr = pair.transport_b.local_addr().unwrap();
    let connection = pair
        .transport_a
        .ensure_connection(&DialPeer {
            agent_id: agent_b.clone(),
            addr,
        })
        .await
        .expect("connect");

    // Positive control: an enrolled peer passes the gate; offering the
    // incumbent's own connection resolves to Existing without changes.
    let admitted = pair
        .transport_a
        .registry
        .admit_gated(
            agent_b.clone(),
            connection.clone(),
            Direction::Outbound,
            || async { true },
        )
        .await;
    assert!(matches!(admitted, Admission::Existing(_)));

    // A refused gate (e.g., revoked between handshake and admission) closes
    // the offered connection and installs nothing.
    let refused = pair
        .transport_a
        .registry
        .admit_gated(
            agent_b.clone(),
            connection.clone(),
            Direction::Outbound,
            || async { false },
        )
        .await;
    assert!(matches!(refused, Admission::Rejected));
}

#[tokio::test]
async fn revoked_peer_is_closed_and_never_redialled() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();

    let first = Envelope::new(
        agent_a.clone(),
        agent_b.clone(),
        MessageKind::Message,
        json!({"n": 1}),
    );
    pair.transport_a
        .send_to(&pair.directory_a, &agent_b, first, Duration::from_secs(5))
        .await
        .expect("initial send");
    assert!(pair.transport_a.has_connection(&agent_b).await);

    // Revoke exactly as the daemon does: commit the directory change, then
    // close the live slot.
    pair.directory_a.remove_peer(&agent_b).await.unwrap();
    pair.transport_a.close_peer(&agent_b, b"peer revoked").await;
    assert!(!pair.transport_a.has_connection(&agent_b).await);

    // Neither an explicit send nor reconnect maintenance may re-establish
    // the revoked peer.
    let second = Envelope::new(
        agent_a,
        agent_b.clone(),
        MessageKind::Message,
        json!({"n": 2}),
    );
    assert!(
        pair.transport_a
            .send_to(&pair.directory_a, &agent_b, second, Duration::from_secs(5))
            .await
            .is_err(),
        "revoked peer must be unreachable"
    );
    pair.transport_a.maintain(&pair.directory_a).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !pair.transport_a.has_connection(&agent_b).await,
        "maintenance must not redial a revoked peer"
    );
}

#[tokio::test]
async fn stale_retirement_spares_current_slot() {
    let pair = make_transport_pair().await;
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr = pair.transport_b.local_addr().unwrap();

    let first = pair
        .transport_a
        .ensure_connection(&DialPeer {
            agent_id: agent_b.clone(),
            addr,
        })
        .await
        .expect("first connection");
    pair.transport_a.close_peer(&agent_b, b"refresh").await;
    let replacement = pair
        .transport_a
        .ensure_connection(&DialPeer {
            agent_id: agent_b.clone(),
            addr,
        })
        .await
        .expect("replacement connection");
    assert_ne!(
        first.stable_id(),
        replacement.stable_id(),
        "fixture requires distinct connections"
    );

    // A failure reported against the retired connection must leave the
    // current authoritative slot untouched — this is the exact property
    // whose violation let a retry destroy a healthy replacement.
    pair.transport_a
        .registry
        .retire_if_current_connection(&agent_b, &first, b"stale send failure")
        .await;
    assert!(
        pair.transport_a.has_connection(&agent_b).await,
        "stale retirement destroyed the current slot"
    );

    // Retiring by the current connection still works.
    pair.transport_a
        .registry
        .retire_if_current_connection(&agent_b, &replacement, b"current send failure")
        .await;
    assert!(!pair.transport_a.has_connection(&agent_b).await);
}

#[tokio::test]
async fn close_peer_cancels_in_flight_dial_tokens() {
    // The IPC revocation contract requires cancelling attempts, not just
    // refusing their admission: close_peer must retire the per-peer dial
    // token so handshake awaits and reconnect dials observe cancellation.
    let pair = make_transport_pair().await;
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();

    let token = pair.transport_a.dial_token(&agent_b).await;
    assert!(!token.is_cancelled());

    pair.transport_a.close_peer(&agent_b, b"peer revoked").await;
    assert!(
        token.is_cancelled(),
        "close_peer must cancel the per-peer dial token"
    );

    // A subsequent dial installs a fresh, uncancelled token.
    let fresh = pair.transport_a.dial_token(&agent_b).await;
    assert!(!fresh.is_cancelled());
}
