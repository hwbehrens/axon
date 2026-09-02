use std::time::Duration;

use serde_json::json;

use super::*;

pub(super) fn agent(value: char) -> AgentId {
    AgentId::parse(&format!("ed25519.{}", value.to_string().repeat(32))).expect("valid Agent ID")
}

pub(super) fn request() -> Arc<Envelope> {
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

    broker.register(1, None).await.expect("first handler");

    assert_eq!(
        broker.register(2, None).await,
        Err(BrokerError::HandlerBusy)
    );
    broker
        .register(1, None)
        .await
        .expect("same handler is idempotent");
}

#[tokio::test]
async fn no_handler_returns_immediate_unhandled_error() {
    let broker = RequestBroker::new(agent('b'));
    let request = request();
    let request_id = request.id;

    let BeginRequest::Respond(response) = broker.begin(request, REQUEST_TTL).await else {
        panic!("request should not be delivered without a handler");
    };

    assert_eq!(response.kind, MessageKind::Error);
    let payload = response.payload_value().expect("payload");
    assert_eq!(payload["code"], "unhandled");
    assert_eq!(
        payload["message"],
        json!(format!(
            "no application handler registered for request '{}'",
            request_id
        ))
    );
    assert_eq!(payload["retryable"], json!(false));
}

#[tokio::test]
async fn handler_can_reply_exactly_once() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1, None).await.expect("handler");
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
    broker.register(1, None).await.expect("handler");
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
        .register(2, None)
        .await
        .expect("new handler after disconnect");
}

#[tokio::test]
async fn non_handler_cannot_reply() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1, None).await.expect("handler");
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
    broker.register(1, None).await.expect("handler");
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
    broker.register(1, None).await.expect("handler");
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
pub(super) const REQUEST_TTL: Duration = Duration::from_secs(30);

#[tokio::test]
async fn oversized_reply_is_rejected_without_consuming_the_request() {
    let broker = RequestBroker::new(agent('a'));
    broker.register(1, None).await.unwrap();
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
    broker.register(1, None).await.unwrap();
    let BeginRequest::Deliver(delivery) = broker.begin(request(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };

    let mut live = std::collections::HashSet::new();
    live.insert(delivery.client_id + 1); // handler vanished
    broker.reconcile_clients(&live).await;

    // Lease is freed: a new client can acquire it.
    assert!(broker.register(2, None).await.is_ok());
    // The orphaned pending request resolves with an explicit error.
    let response = delivery.response.await.expect("orphaned request resolves");
    assert_eq!(response.kind, MessageKind::Error);
}

// =========================================================================
// Round-three review regressions: terminal-outcome tombstones and
// replay-before-handler-lookup.
// =========================================================================

// =========================================================================
// Round-five review regressions: peer-scoped request correlation.
// =========================================================================
