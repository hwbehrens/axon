use super::fixtures::{make_transport_pair, make_transport_pair_with_options, peer_record};
use crate::message::{Envelope, MessageKind};
use crate::transport::{ResponseHandlerFn, default_error_response};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn send_request_bidirectional_default_error() {
    let pair = make_transport_pair().await;
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");
    let peer_b = peer_record(&pair.id_b, addr_b);

    let request = Envelope::new(
        pair.id_a.agent_id().to_string(),
        pair.id_b.agent_id().to_string(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );

    let result = pair
        .transport_a
        .send(&peer_b, request.clone())
        .await
        .expect("send");
    let response = result.expect("expected response");
    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(response.ref_id, Some(request.id));
}

#[tokio::test]
async fn send_request_rejects_invalid_bidirectional_reply() {
    let handler: ResponseHandlerFn = Arc::new(|request| {
        Box::pin(async move {
            Some(Envelope::new(
                request.to.as_deref().unwrap().to_string(),
                request.from.as_deref().unwrap().to_string(),
                MessageKind::Message,
                json!({"unexpected": true}),
            ))
        })
    });
    let pair = make_transport_pair_with_options(128, 128, Some(handler)).await;
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");
    let peer_b = peer_record(&pair.id_b, addr_b);

    let request = Envelope::new(
        pair.id_a.agent_id().to_string(),
        pair.id_b.agent_id().to_string(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );

    let err = pair
        .transport_a
        .send(&peer_b, request)
        .await
        .expect_err("invalid bidirectional reply should be rejected");
    assert!(
        err.to_string()
            .contains("bidirectional reply must use response|error kind")
    );
}

#[tokio::test]
async fn send_with_timeout_honors_custom_request_timeout() {
    let handler: ResponseHandlerFn = Arc::new(|request| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Some(default_error_response(
                &request,
                request.to.as_deref().unwrap(),
            ))
        })
    });
    let pair = make_transport_pair_with_options(128, 128, Some(handler)).await;
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");
    let peer_b = peer_record(&pair.id_b, addr_b);

    let request = Envelope::new(
        pair.id_a.agent_id().to_string(),
        pair.id_b.agent_id().to_string(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );

    let err = pair
        .transport_a
        .send_with_timeout(&peer_b, request, Duration::from_millis(50))
        .await
        .expect_err("custom request timeout should be enforced");
    assert!(err.to_string().contains("request timed out after 50ms"));
}
