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

    let BeginRequest::Respond(response) = broker.begin(request()).await else {
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
    let BeginRequest::Deliver(delivery) = broker.begin(original.clone()).await else {
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
    let BeginRequest::Deliver(delivery) = broker.begin(request()).await else {
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
    let BeginRequest::Deliver(_delivery) = broker.begin(original.clone()).await else {
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
    let BeginRequest::Deliver(delivery) = broker.begin(request()).await else {
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
        let BeginRequest::Deliver(delivery) = broker.begin(Arc::new(next)).await else {
            panic!("request within capacity should be delivered");
        };
        deliveries.push(delivery);
    }

    let mut overflow = (*request()).clone();
    overflow.id = uuid::Uuid::from_u128((MAX_PENDING_REQUESTS + 1) as u128);
    let BeginRequest::Respond(response) = broker.begin(Arc::new(overflow)).await else {
        panic!("request above capacity should be rejected");
    };

    assert_eq!(
        response.payload_value().expect("payload")["code"],
        "overloaded"
    );
    assert_eq!(broker.pending_count().await, MAX_PENDING_REQUESTS);
    drop(deliveries);
}
