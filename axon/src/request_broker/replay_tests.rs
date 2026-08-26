//! Replay / dedup / peer-scoped-correlation coverage for [`RequestBroker`].
//!
//! Split from `tests.rs` for file-length limits.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::tests::{REQUEST_TTL, agent, request};
use super::*;

#[tokio::test]
async fn retried_request_replays_cached_completion_without_reexecution() {
    let broker = RequestBroker::new(agent('a'));
    broker.register(1).await.unwrap();

    let original = request();
    let BeginRequest::Deliver(delivery) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("first delivery expected");
    };
    broker
        .reply(
            delivery.client_id,
            delivery.request_id,
            MessageKind::Response,
            json!({"answer": 42}),
            None,
        )
        .await
        .expect("reply accepted");

    // The transport-level retry of the same envelope must not reach the
    // application handler again: it replays the cached response.
    let BeginRequest::Respond(replay) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("cached replay expected");
    };
    assert!(
        replay.payload.get().contains("42"),
        "unexpected replay payload: {}",
        replay.payload.get()
    );
}
#[tokio::test]
async fn duplicate_of_inflight_request_is_rejected_retryable() {
    let broker = RequestBroker::new(agent('a'));
    broker.register(1).await.unwrap();

    let original = request();
    let BeginRequest::Deliver(first) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };

    let BeginRequest::Respond(duplicate) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("duplicate rejection expected");
    };
    assert!(
        duplicate.payload.get().contains("in-flight"),
        "unexpected duplicate response: {}",
        duplicate.payload.get()
    );
    drop(first);
}
#[tokio::test]
async fn cancelled_deliveries_are_swept_once_the_ttl_expires() {
    // Simulates QUIC connection loss dropping the awaiting task: the
    // delivery (and its resolver) is discarded without replying.
    let broker = RequestBroker::new(agent('a'));
    broker.register(1).await.unwrap();
    let original = request();
    let BeginRequest::Deliver(delivery) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };
    drop(delivery); // cancelled before any reply

    // Before the TTL the entry still occupies its slot...
    let BeginRequest::Respond(early) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("duplicate rejection expected while pending");
    };
    assert!(early.payload.get().contains("in-flight"));

    // ...and once the TTL lapses, the lazy sweep frees the slot and
    // tombstones the terminal timeout: redelivery of the same UUID replays
    // that outcome instead of opening a fresh delivery (a stale handler's
    // late reply must never satisfy a new exchange).
    tokio::time::sleep(Duration::from_millis(5)).await;
    let BeginRequest::Respond(replayed) = broker.begin(original.clone(), Duration::ZERO).await
    else {
        panic!("swept request must replay its tombstoned terminal outcome");
    };
    assert_eq!(replayed.payload_value().unwrap()["code"], "timeout");
}
#[tokio::test]
async fn same_call_retry_after_sweep_replays_tombstone_not_fresh_delivery() {
    // Regression: a single begin() call both sweeps the expired prior
    // attempt of THIS UUID and then inserts. Without the post-sweep tombstone
    // recheck, that call produced a fresh delivery whose pending entry the
    // stale attempt's late reply could satisfy.
    let broker = RequestBroker::new(agent('a'));
    broker.register(1).await.unwrap();
    let original = request();
    let tiny_ttl = Duration::from_millis(50);
    let BeginRequest::Deliver(first) = broker.begin(original.clone(), tiny_ttl).await else {
        panic!("delivery expected");
    };
    drop(first); // cancelled before any reply

    tokio::time::sleep(tiny_ttl + Duration::from_millis(100)).await;
    // SAME call sweeps the expired entry and evaluates this UUID:
    let outcome = broker.begin(original.clone(), tiny_ttl).await;
    let BeginRequest::Respond(replayed) = outcome else {
        panic!("same-call retry must not become a fresh delivery");
    };
    assert_eq!(replayed.payload_value().unwrap()["code"], "timeout");
    // The late reply can only hit RequestNotFound: nothing is pending.
    assert_eq!(
        broker
            .reply(
                1,
                original.id,
                MessageKind::Response,
                json!({"late": true}),
                None
            )
            .await,
        Err(BrokerError::RequestNotFound)
    );
}
#[tokio::test]
async fn swept_request_uuid_is_tombstoned_and_never_redelivered() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.expect("handler");
    let original = request();
    let tiny_ttl = Duration::from_millis(50);
    let BeginRequest::Deliver(first) = broker.begin(original.clone(), tiny_ttl).await else {
        panic!("request should be delivered");
    };

    // Force the lazy sweep to expire the pending entry while handler 1 is
    // still connected (simulates a cancelled awaiter).
    tokio::time::sleep(tiny_ttl + Duration::from_millis(100)).await;
    let other = Arc::new(Envelope::new(
        agent('a'),
        agent('b'),
        MessageKind::Request,
        json!({"question":"other"}),
    ));
    // Any begin() runs the lazy sweep; its own outcome is irrelevant here.
    // Any begin() runs the lazy sweep; its own outcome is irrelevant here.
    // It must pass the same tiny TTL for the expiry comparison.
    let outcome_other = broker.begin(other, tiny_ttl).await;
    drop(outcome_other);
    // Observing the swept entry's terminal response keeps the sender side
    // of the oneshot from being reported dropped.
    let _ = first.response.await;

    // Redelivery of the SAME envelope id must replay the tombstoned
    // timeout, not open a new delivery that the old handler's late reply
    // could satisfy.
    let outcome = broker.begin(original.clone(), REQUEST_TTL).await;
    let BeginRequest::Respond(replayed) = outcome else {
        panic!("tombstoned request must not be redelivered");
    };
    assert_eq!(replayed.payload_value().unwrap()["code"], "timeout");

    // The stale handler's late reply can only hit RequestNotFound.
    assert_eq!(
        broker
            .reply(
                1,
                original.id,
                MessageKind::Response,
                json!({"late": true}),
                None
            )
            .await,
        Err(BrokerError::RequestNotFound)
    );
}
#[tokio::test]
async fn completed_response_replays_without_a_registered_handler() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.expect("handler");
    let original = request();
    let BeginRequest::Deliver(delivery) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("request should be delivered");
    };
    broker
        .reply(
            1,
            original.id,
            MessageKind::Response,
            json!({"answer":"cached"}),
            None,
        )
        .await
        .expect("reply");
    let _ = broker
        .await_response(delivery, Duration::from_secs(1))
        .await;

    // Handler disconnects (client gone), then the peer retries the same
    // request: the cached response must win over `unhandled`.
    broker.disconnect(1).await;
    let BeginRequest::Respond(replayed) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("completed exchange must not be re-delivered after handler loss");
    };
    assert_eq!(replayed.kind, MessageKind::Response);
    assert_eq!(replayed.payload_value().unwrap()["answer"], json!("cached"));
}
/// A request envelope identical to [`request`] but originating from a
/// different authenticated peer, optionally reusing the same UUID.
fn request_from(peer: AgentId, id: Uuid) -> Arc<Envelope> {
    let mut envelope = Envelope::new(
        peer,
        agent('b'),
        MessageKind::Request,
        json!({"question":"ready?"}),
    );
    envelope.id = id;
    Arc::new(envelope)
}
#[tokio::test]
async fn same_uuid_from_another_peer_does_not_replay_cached_response() {
    // Regression: pending entries and completed-response cache were keyed by
    // UUID alone, so a second peer reusing a UUID could replay the first
    // peer's cached response without its request ever being executed.
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.unwrap();

    let original = request();
    let BeginRequest::Deliver(first) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };
    broker
        .reply(
            1,
            original.id,
            MessageKind::Response,
            json!({"answer": "from-a"}),
            None,
        )
        .await
        .expect("reply accepted");
    let _ = broker.await_response(first, Duration::from_secs(1)).await;

    // A DIFFERENT peer presents the SAME UUID: it must be delivered fresh,
    // never answered from peer 'a's cached completion.
    let other_peer = request_from(agent('c'), original.id);
    let BeginRequest::Deliver(second) = broker.begin(other_peer, REQUEST_TTL).await else {
        panic!("same-UUID request from another peer must not hit the cache");
    };
    assert_eq!(second.request_id, original.id);
    drop(second);

    // And the first peer's redelivery still replays ITS own outcome.
    let BeginRequest::Respond(replay) = broker.begin(original, REQUEST_TTL).await else {
        panic!("cached replay expected for the original peer");
    };
    assert_eq!(replay.payload_value().unwrap()["answer"], json!("from-a"));
}
#[tokio::test]
async fn uuid_collision_reply_is_ambiguous_without_peer_and_scoped_with_it() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.unwrap();

    let shared_id = uuid::Uuid::from_u128(0xfeed);
    let from_a = request_from(agent('a'), shared_id);
    let from_c = request_from(agent('c'), shared_id);
    let BeginRequest::Deliver(delivery_a) = broker.begin(from_a, REQUEST_TTL).await else {
        panic!("delivery expected");
    };
    let BeginRequest::Deliver(delivery_c) = broker.begin(from_c, REQUEST_TTL).await else {
        panic!("second peer's same-UUID request must pend independently");
    };

    // Ambiguous: two peers have this UUID pending and no peer was supplied.
    assert_eq!(
        broker
            .reply(1, shared_id, MessageKind::Response, json!({}), None)
            .await,
        Err(BrokerError::AmbiguousRequest)
    );
    assert_eq!(broker.pending_count().await, 2);

    // Peer-scoped replies resolve exactly their own exchange.
    broker
        .reply(
            1,
            shared_id,
            MessageKind::Response,
            json!({"who": "a"}),
            Some(agent('a')),
        )
        .await
        .expect("scoped reply to peer a");
    broker
        .reply(
            1,
            shared_id,
            MessageKind::Response,
            json!({"who": "c"}),
            Some(agent('c')),
        )
        .await
        .expect("scoped reply to peer c");

    let response_a = delivery_a.response.await.expect("peer a response");
    let response_c = delivery_c.response.await.expect("peer c response");
    assert_eq!(response_a.payload_value().unwrap()["who"], json!("a"));
    assert_eq!(response_c.payload_value().unwrap()["who"], json!("c"));
}
