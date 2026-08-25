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
            .reply(1, original.id, MessageKind::Response, json!({}))
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
        .reply(2, original.id, MessageKind::Response, json!({}))
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

    // ...and once the TTL lapses, the lazy sweep frees the slot: the same
    // request can be delivered to the handler again.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let BeginRequest::Deliver(redelivered) = broker.begin(original.clone(), Duration::ZERO).await
    else {
        panic!("swept request must be deliverable again");
    };
    assert_eq!(redelivered.request_id, original.id);
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
            .reply(1, original.id, MessageKind::Response, json!({"late": true}))
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
