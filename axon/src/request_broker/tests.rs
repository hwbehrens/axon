use std::time::Duration;

use serde_json::json;

use super::*;

fn agent(value: char) -> AgentId {
    AgentId::parse(&format!("ed25519.{}", value.to_string().repeat(32))).expect("valid Agent ID")
}

fn request() -> Arc<Envelope> {
    Arc::new(Envelope::new(
        agent('a'),
        agent('b'),
        MessageKind::Request,
        json!({"question":"ready?"}),
    ))
}

#[tokio::test]
async fn one_connection_owns_handler_lease() {
    let broker = RequestBroker::new(agent('b'));

    broker.register(1).await.expect("first handler");

    assert_eq!(broker.register(2).await, Err(BrokerError::HandlerBusy));
    broker
        .register(1)
        .await
        .expect("same handler is idempotent");
}

#[tokio::test]
async fn no_handler_returns_immediate_unhandled_error() {
    let broker = RequestBroker::new(agent('b'));

    let BeginRequest::Respond(response) = broker.begin(request(), REQUEST_TTL).await else {
        panic!("request should not be delivered without a handler");
    };

    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(
        response.payload_value().expect("payload")["code"],
        "unhandled"
    );
}

#[tokio::test]
async fn handler_can_reply_exactly_once() {
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
            json!({"answer":"yes"}),
            None,
        )
        .await
        .expect("first reply");
    let response = broker
        .await_response(delivery, std::time::Duration::from_secs(1))
        .await;

    assert_eq!(response.ref_id, Some(original.id));
    assert_eq!(response.kind, MessageKind::Response);
    assert_eq!(
        broker
            .reply(1, original.id, MessageKind::Response, json!({}), None)
            .await,
        Err(BrokerError::RequestNotFound)
    );
}

#[tokio::test]
async fn disconnect_releases_lease_and_terminates_pending_requests() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.expect("handler");
    let BeginRequest::Deliver(delivery) = broker.begin(request(), REQUEST_TTL).await else {
        panic!("request should be delivered");
    };

    broker.disconnect(1).await;
    let response = broker
        .await_response(delivery, std::time::Duration::from_secs(1))
        .await;

    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(
        response.payload_value().expect("payload")["code"],
        "unhandled"
    );
    broker
        .register(2)
        .await
        .expect("new handler after disconnect");
}

#[tokio::test]
async fn non_handler_cannot_reply() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.expect("handler");
    let original = request();
    let BeginRequest::Deliver(_delivery) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("request should be delivered");
    };

    let result = broker
        .reply(2, original.id, MessageKind::Response, json!({}), None)
        .await;

    assert_eq!(result, Err(BrokerError::NotHandler));
    assert_eq!(broker.pending_count().await, 1);
}

#[tokio::test]
async fn handler_deadline_removes_pending_request() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.expect("handler");
    let BeginRequest::Deliver(delivery) = broker.begin(request(), REQUEST_TTL).await else {
        panic!("request should be delivered");
    };

    let response = broker
        .await_response(delivery, std::time::Duration::from_millis(1))
        .await;

    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(
        response.payload_value().expect("payload")["code"],
        "timeout"
    );
    assert_eq!(broker.pending_count().await, 0);
}

#[tokio::test]
async fn pending_request_capacity_is_bounded() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1).await.expect("handler");
    let mut deliveries = Vec::new();
    for index in 0..MAX_PENDING_REQUESTS {
        let mut next = (*request()).clone();
        next.id = uuid::Uuid::from_u128(index as u128 + 1);
        let BeginRequest::Deliver(delivery) = broker.begin(Arc::new(next), REQUEST_TTL).await
        else {
            panic!("request within capacity should be delivered");
        };
        deliveries.push(delivery);
    }

    let mut overflow = (*request()).clone();
    overflow.id = uuid::Uuid::from_u128((MAX_PENDING_REQUESTS + 1) as u128);
    let BeginRequest::Respond(response) = broker.begin(Arc::new(overflow), REQUEST_TTL).await
    else {
        panic!("request above capacity should be rejected");
    };

    assert_eq!(
        response.payload_value().expect("payload")["code"],
        "overloaded"
    );
    assert_eq!(broker.pending_count().await, MAX_PENDING_REQUESTS);
    drop(deliveries);
}

/// Broker tests use the daemon's production deadline so sweep semantics
/// cannot drift from what await_response enforces.
const REQUEST_TTL: Duration = Duration::from_secs(30);

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
async fn oversized_reply_is_rejected_without_consuming_the_request() {
    let broker = RequestBroker::new(agent('a'));
    broker.register(1).await.unwrap();
    let original = request();
    let BeginRequest::Deliver(delivery) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };

    // A payload object that encodes past the 65,536-byte wire limit.
    let oversized = json!({"blob": "x".repeat(MAX_MESSAGE_SIZE as usize)});
    assert_eq!(
        broker
            .reply(1, original.id, MessageKind::Response, oversized, None)
            .await,
        Err(BrokerError::InvalidPayload),
        "over-limit reply must be rejected at IPC, not dropped on QUIC"
    );

    // The request is still pending: a smaller reply succeeds.
    broker
        .reply(
            1,
            original.id,
            MessageKind::Response,
            json!({"answer": "fits"}),
            None,
        )
        .await
        .expect("reply within the wire limit must be accepted");
    let response = delivery.response.await.expect("terminal response");
    assert_eq!(response.payload_value().unwrap()["answer"], "fits");
}

#[tokio::test]
async fn reconcile_clients_revokes_leases_and_pending_for_gone_clients() {
    let broker = RequestBroker::new(agent('a'));
    broker.register(1).await.unwrap();
    let BeginRequest::Deliver(delivery) = broker.begin(request(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };

    let mut live = std::collections::HashSet::new();
    live.insert(delivery.client_id + 1); // handler vanished
    broker.reconcile_clients(&live).await;

    // Lease is freed: a new client can acquire it.
    assert!(broker.register(2).await.is_ok());
    // The orphaned pending request resolves with an explicit error.
    let response = delivery.response.await.expect("orphaned request resolves");
    assert_eq!(response.kind, MessageKind::Error);
}

// =========================================================================
// Round-three review regressions: terminal-outcome tombstones and
// replay-before-handler-lookup.
// =========================================================================

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

// =========================================================================
// Round-five review regressions: peer-scoped request correlation.
// =========================================================================

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
