use std::time::Duration;

use serde_json::json;

use super::*;
use crate::message::MAX_MESSAGE_SIZE;

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
async fn timeout_replays_a_reply_recorded_after_the_deadline_fired() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1, None).await.expect("handler");
    let original = request();
    let BeginRequest::Deliver(delivery) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };

    // Reproduce the window between the deadline firing (which drops the
    // receiver) and this branch taking the state lock: the handler's reply
    // critical section has completed and recorded its outcome.
    let handler_response = Envelope::response_to(
        &original,
        agent('b'),
        MessageKind::Response,
        json!({"answer": "recorded"}),
    );
    let pending = {
        let mut state = broker.state.lock().await;
        let pending = state.pending.remove(&delivery.key).expect("pending entry");
        state
            .completed
            .remember(delivery.key.clone(), handler_response.clone());
        pending
    };
    // Keep the handler's sender alive: dropping it without sending would
    // close the receiver and take the disconnected branch instead of the
    // deadline this test pins.
    let _handler_side = pending;

    let response = broker
        .await_response(delivery, Duration::from_millis(1))
        .await;

    assert_eq!(
        response, handler_response,
        "a recorded handler outcome must win over a divergent timeout"
    );
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
async fn wire_legal_reply_that_overflows_the_ipc_frame_is_rejected() {
    let broker = RequestBroker::new(agent('a'));
    broker.register(1, None).await.unwrap();
    let original = request();
    let BeginRequest::Deliver(delivery) = broker.begin(original.clone(), REQUEST_TTL).await else {
        panic!("delivery expected");
    };

    // A response envelope sized exactly at the QUIC wire limit. The IPC-only
    // `from`/`to` routing fields plus the requester's `send` reply wrapper
    // push the framed line past the 65,536-byte IPC bound.
    let base = Envelope::response_to(
        &original,
        agent('a'),
        MessageKind::Response,
        json!({"blob": ""}),
    );
    let base_wire = base.wire_encode().unwrap().len();
    let payload = json!({"blob": "x".repeat(MAX_MESSAGE_SIZE as usize - base_wire)});
    let at_wire_limit = Envelope::response_to(
        &original,
        agent('a'),
        MessageKind::Response,
        payload.clone(),
    );
    assert_eq!(
        at_wire_limit.wire_encode().unwrap().len(),
        MAX_MESSAGE_SIZE as usize
    );
    assert!(
        serde_json::to_vec(&at_wire_limit).unwrap().len() > crate::ipc::MAX_IPC_LINE_LENGTH,
        "fixture must overflow the IPC frame"
    );

    assert_eq!(
        broker
            .reply(1, original.id, MessageKind::Response, payload, None)
            .await,
        Err(BrokerError::InvalidPayload),
        "an IPC-frame overflow must be rejected before the handler is acked"
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
        .expect("reply within both bounds must be accepted");
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
