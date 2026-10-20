use super::fixtures::{make_transport_pair, make_transport_pair_with_options};
use crate::message::{AgentId, Envelope, MessageKind};
use crate::transport::{DialPeer, ResponseHandlerFn, default_error_response};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test]
async fn send_request_bidirectional_default_error() {
    let pair = make_transport_pair().await;
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();

    let request = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );

    let result = pair
        .transport_a
        .send_to(
            &pair.directory_a,
            &peer_b,
            request.clone(),
            Duration::from_secs(5),
        )
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
                request.to.clone().unwrap(),
                request.from.clone().unwrap(),
                MessageKind::Message,
                json!({"unexpected": true}),
            ))
        })
    });
    let pair = make_transport_pair_with_options(128, 128, Some(handler)).await;
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();

    let request = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );

    let err = pair
        .transport_a
        .send_to(&pair.directory_a, &peer_b, request, Duration::from_secs(5))
        .await
        .expect_err("invalid bidirectional reply should be rejected");
    eprintln!("DEBUG err: {err:#}");
    assert!(
        err.to_string()
            .contains("bidirectional reply must use response|error kind")
    );
    assert!(
        !pair.transport_a.has_connection(&peer_b).await,
        "failed exchange must invalidate the connection slot"
    );
}

/// The spec-compliance test `describe_on_legacy_peer_yields_unsupported_kind`
/// constructs the mandated reply itself; this drives the transport's own
/// `handle_authenticated_bidi` unknown-kind branch end to end.
#[tokio::test]
async fn unknown_bidi_kind_is_answered_with_unsupported_kind() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr = pair.transport_b.local_addr().unwrap();
    let connection = pair
        .transport_a
        .ensure_connection(&DialPeer {
            agent_id: peer_b.clone(),
            addr,
        })
        .await
        .expect("connect");

    let request = Envelope::new(
        agent_a.clone(),
        peer_b,
        MessageKind::unknown("future_kind_v99"),
        json!({"future": true}),
    );
    let response = crate::transport::connection::send_request(
        &connection,
        request.clone(),
        &agent_a,
        Instant::now() + Duration::from_secs(5),
    )
    .await
    .expect("unknown kind must be answered, not dropped");

    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(response.ref_id, Some(request.id));
    let payload = response.payload_value().unwrap();
    assert_eq!(payload["code"], json!("unsupported_kind"));
    assert!(
        payload["message"]
            .as_str()
            .unwrap()
            .contains("future_kind_v99"),
        "reply must name the original bounded kind: {payload}"
    );
}

/// Producer-level pin for the post-connect, pre-write phase: `send_request`
/// checks the remaining budget before opening the bidi stream on an ALREADY
/// connected peer. Exhaustion there must be non-ambiguous and keep the
/// stream-open phase detail, so the daemon neither claims the peer was never
/// reached nor drops the inner reason, and a fire-and-forget retry would
/// still be at-most-once.
#[tokio::test]
async fn expired_budget_fails_before_the_bidi_stream_open_without_ambiguity() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr = pair.transport_b.local_addr().unwrap();
    let connection = pair
        .transport_a
        .ensure_connection(&DialPeer {
            agent_id: peer_b.clone(),
            addr,
        })
        .await
        .expect("connect");
    assert!(
        pair.transport_a.has_connection(&peer_b).await,
        "the peer must be connected before the stream-open phase begins"
    );

    let request = Envelope::new(
        agent_a.clone(),
        peer_b,
        MessageKind::Request,
        json!({"question": "budget already gone?"}),
    );
    let err = crate::transport::connection::send_request(
        &connection,
        request,
        &agent_a,
        Instant::now() - Duration::from_secs(1),
    )
    .await
    .expect_err("an exhausted budget must fail");

    assert!(
        err.timed_out,
        "budget exhaustion must stay timed_out: {err}"
    );
    assert!(
        !err.ambiguous,
        "no payload byte can have been written before the stream opened: {err}"
    );
    assert!(
        err.to_string().contains("bidi stream open"),
        "the daemon needs the stream-open phase detail: {err}"
    );
}

/// Producer-level pin for the in-flight open boundary: once the peer's bidi
/// stream-credit window is full, `open_bi` awaits and the whole-exchange
/// budget can expire AFTER the pre-check passed but BEFORE a stream exists.
/// A timed-out open returns no stream and `write_framed` never ran, so the
/// failure must stay non-ambiguous and keep the open phase detail.
#[tokio::test]
async fn in_flight_bidi_open_timeout_is_classified_pre_send() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr = pair.transport_b.local_addr().unwrap();
    let connection = pair
        .transport_a
        .ensure_connection(&DialPeer {
            agent_id: peer_b.clone(),
            addr,
        })
        .await
        .expect("connect");

    // Exhaust the peer-advertised bidi stream credit (8) and hold both
    // halves of each stream: the peer replenishes MAX_STREAMS only when its
    // receive side releases a stream, and its handlers are blocked reading
    // our never-finished frames.
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(connection.open_bi().await.expect("bidi stream credit"));
    }

    let request = Envelope::new(
        agent_a.clone(),
        peer_b,
        MessageKind::Request,
        json!({ "question": "any credit left?" }),
    );
    let err = crate::transport::connection::send_request(
        &connection,
        request,
        &agent_a,
        Instant::now() + Duration::from_millis(300),
    )
    .await
    .expect_err("open must not complete while the credit window is full");

    assert!(
        err.timed_out,
        "budget exhaustion must stay timed_out: {err}"
    );
    assert!(
        !err.ambiguous,
        "a timed-out open wrote no payload byte: {err}"
    );
    assert!(
        err.to_string().contains("bidi stream open"),
        "the daemon needs the open phase detail: {err}"
    );
    drop(held);
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
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();

    let request = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );

    let err = pair
        .transport_a
        .send_to(
            &pair.directory_a,
            &peer_b,
            request,
            Duration::from_millis(50),
        )
        .await
        .expect_err("custom request timeout should be enforced");
    // The whole exchange (including the refresh-and-retry dial) is bounded
    // by the one budget, so exhaustion may surface either as the response
    // wait timing out or as a refused redial — both are timeout failures.
    assert!(err.timed_out, "expected a timeout failure, got: {err}");
    assert!(
        !pair.transport_a.has_connection(&peer_b).await,
        "timed-out exchange must invalidate the connection slot"
    );
}
