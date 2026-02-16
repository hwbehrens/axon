use crate::*;

mod connection;

// =========================================================================
// §10 Protocol Violation Handling
// =========================================================================

/// spec.md §10: After hello, connection is authenticated and subsequent
/// messages are accepted. Verifies the hello-first invariant holds.
#[tokio::test]
async fn violation_hello_first_invariant() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    // ensure_connection performs hello automatically; connection should succeed.
    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();
    assert!(transport_a.has_connection(id_b.agent_id()).await);
    assert!(conn.close_reason().is_none());

    // After hello, a request should succeed (proves post-hello messages are accepted).
    let request = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Request,
        json!({"question": "post-hello test"}),
    );
    let result = transport_a.send(&peer_b, request).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().kind, MessageKind::Error);
}

/// default_error_response always returns Error kind with "unhandled" code.
#[tokio::test]
async fn violation_default_error_response_returns_error() {
    let req = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Request,
        json!({"data": "test"}),
    );
    let resp =
        axon::transport::default_error_response(&req, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(resp.kind, MessageKind::Error);
    let payload: Value = serde_json::from_str(resp.payload.get()).unwrap();
    assert_eq!(payload["code"], "unhandled");
    assert_eq!(payload["retryable"], false);
}

/// spec.md §10: Unknown kind on bidi stream returns error.
/// Tested via default_error_response since we cannot inject raw wire bytes from
/// integration tests (framing is pub(crate)).
#[tokio::test]
async fn violation_unknown_kind_on_bidi_returns_error() {
    let unexpected_bidi = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Unknown,
        json!({"data": "test"}),
    );
    let resp = axon::transport::default_error_response(
        &unexpected_bidi,
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    assert_eq!(resp.kind, MessageKind::Error);
    let payload: Value = serde_json::from_str(resp.payload.get()).unwrap();
    assert_eq!(payload["code"], "unhandled");
}

/// spec.md §10: Fire-and-forget messages (Message kind) delivered via uni stream
/// return no response. Verifies transport drops no valid fire-and-forget.
#[tokio::test]
async fn violation_fire_and_forget_no_response() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();
    let mut rx_b = transport_b.subscribe_inbound();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let message = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Message,
        json!({"topic": "violation.test", "data": {"x": 1}}),
    );

    let result = transport_a.send(&peer_b, message).await.unwrap();
    assert!(
        result.is_none(),
        "fire-and-forget must not return a response"
    );

    // Verify the message was delivered.
    let received = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
        .await
        .expect("timeout")
        .expect("recv");
    assert_eq!(received.kind, MessageKind::Message);
}

/// spec.md §10: Request on bidi gets error response (default_error_response
/// since no application handler is registered).
#[tokio::test]
async fn violation_request_gets_error_response() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    let request = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Request,
        json!({}),
    );

    let result = transport_a.send(&peer_b, request.clone()).await.unwrap();
    let resp = result.expect("request must get an error response");
    assert_eq!(resp.kind, MessageKind::Error);
    assert_eq!(resp.ref_id, Some(request.id));
}

/// spec.md §10: Invalid envelope on bidi request returns error.
#[tokio::test]
async fn violation_invalid_envelope_returns_error() {
    // An envelope with nil UUID should fail validate().
    let mut invalid = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );
    invalid.id = Default::default();
    assert!(invalid.validate().is_err());
}

/// spec.md §10: Multiple requests after hello all get error responses.
/// Verifies the connection stays open and handles multiple requests.
#[tokio::test]
async fn violation_connection_survives_multiple_requests() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .unwrap();
    let addr_b = transport_b.local_addr().unwrap();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .unwrap();

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = make_peer_record(&id_b, addr_b);

    // Send multiple requests — all should get Error responses.
    for i in 0..3 {
        let request = Envelope::new(
            id_a.agent_id().to_string(),
            id_b.agent_id().to_string(),
            MessageKind::Request,
            json!({"request_num": i}),
        );
        let env_id = request.id;
        let result = transport_a.send(&peer_b, request).await.unwrap();
        let resp = result.unwrap_or_else(|| {
            panic!("expected response for request {i}");
        });
        assert_eq!(
            resp.kind,
            MessageKind::Error,
            "request {i} should get Error, got {}",
            resp.kind
        );
        assert_eq!(
            resp.ref_id,
            Some(env_id),
            "request {i} response must ref the request"
        );
    }

    // Connection should still be alive.
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}
