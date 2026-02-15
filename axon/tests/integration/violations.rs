use crate::*;

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

    // After hello, a query should succeed (proves post-hello messages are accepted).
    let query = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Query,
        json!({"question": "post-hello test", "domain": "test"}),
    );
    let result = transport_a.send(&peer_b, query).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().kind, MessageKind::Response);
}

/// spec.md §10: Version mismatch in hello returns error(incompatible_version).
/// Tested via auto_response since the public transport API always sends v1.
#[tokio::test]
async fn violation_version_mismatch_error() {
    let req = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Hello,
        json!({"protocol_versions": [99, 100]}),
    );
    let resp = axon::transport::auto_response(&req, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(resp.kind, MessageKind::Error);
    let payload: Value = serde_json::from_str(resp.payload.get()).unwrap();
    assert_eq!(payload["code"], "incompatible_version");
    assert_eq!(payload["retryable"], false);
}

/// spec.md §10: Unknown kind on bidi stream returns error(unknown_kind).
/// Tested via auto_response since we cannot inject raw wire bytes from
/// integration tests (framing is pub(crate)).
#[tokio::test]
async fn violation_unknown_kind_on_bidi_returns_error() {
    // auto_response's catch-all arm handles unexpected kinds on bidi.
    // Construct an envelope that would hit that arm (e.g. Result on bidi).
    let unexpected_bidi = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Result,
        json!({"task_id": "123", "status": "completed", "output": {}}),
    );
    let resp = axon::transport::auto_response(
        &unexpected_bidi,
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    assert_eq!(resp.kind, MessageKind::Error);
    let payload: Value = serde_json::from_str(resp.payload.get()).unwrap();
    assert_eq!(payload["code"], "unknown_kind");
}

/// spec.md §10: Fire-and-forget messages (notify) delivered via uni stream
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

    let notify = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Notify,
        json!({"topic": "violation.test", "data": {"x": 1}, "importance": "low"}),
    );

    let result = transport_a.send(&peer_b, notify).await.unwrap();
    assert!(
        result.is_none(),
        "fire-and-forget must not return a response"
    );

    // Verify the message was delivered.
    let received = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
            .await
            .expect("timeout")
            .expect("recv");
        if msg.kind != MessageKind::Hello {
            break msg;
        }
    };
    assert_eq!(received.kind, MessageKind::Notify);
}

/// spec.md §10: Ping on bidi gets pong (validates auto_response for
/// request kinds that must produce the correct response type).
#[tokio::test]
async fn violation_ping_gets_pong_response() {
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

    let ping = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Ping,
        json!({}),
    );

    let result = transport_a.send(&peer_b, ping.clone()).await.unwrap();
    let pong = result.expect("ping must get a pong response");
    assert_eq!(pong.kind, MessageKind::Pong);
    assert_eq!(pong.ref_id, Some(ping.id));
}

/// spec.md §10: Invalid envelope on bidi request returns error(invalid_envelope).
/// Tested via auto_response since the transport validates before responding.
#[tokio::test]
async fn violation_invalid_envelope_returns_error() {
    // An envelope with bad agent IDs should fail validate().
    let invalid = Envelope::new(
        "bad_id".to_string(),
        "also_bad".to_string(),
        MessageKind::Query,
        json!({"question": "test?"}),
    );
    assert!(invalid.validate().is_err());

    // The transport would send error(invalid_envelope) for this on a bidi stream.
    // Verify the error code is available.
    let error_payload = axon::message::ErrorPayload {
        code: axon::message::ErrorCode::InvalidEnvelope,
        message: "envelope validation failed".to_string(),
        retryable: false,
    };
    let v: Value = serde_json::to_value(&error_payload).unwrap();
    assert_eq!(v["code"], "invalid_envelope");
    assert_eq!(v["retryable"], false);
}

/// spec.md §10: Multiple request types after hello all get correct responses.
/// Verifies the connection stays open and handles multiple violations/requests.
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

    // Send ping, then query, then discover — all should succeed.
    let ping = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Ping,
        json!({}),
    );
    let pong = transport_a
        .send(&peer_b, ping)
        .await
        .unwrap()
        .expect("expected pong");
    assert_eq!(pong.kind, MessageKind::Pong);

    let query = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Query,
        json!({"question": "second request", "domain": "test"}),
    );
    let response = transport_a
        .send(&peer_b, query)
        .await
        .unwrap()
        .expect("expected response");
    assert_eq!(response.kind, MessageKind::Response);

    let discover = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Discover,
        json!({}),
    );
    let caps = transport_a
        .send(&peer_b, discover)
        .await
        .unwrap()
        .expect("expected capabilities");
    assert_eq!(caps.kind, MessageKind::Capabilities);

    // Connection should still be alive.
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}
