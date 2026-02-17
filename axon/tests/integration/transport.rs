use crate::*;

// =========================================================================
// Wire format integration (encode → decode through the full pipeline)
// =========================================================================

/// Envelope survives encode → decode roundtrip for all message kinds.
#[test]
fn envelope_roundtrip_all_kinds() {
    let a = "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string();
    let b = "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string();

    let payloads = vec![
        (MessageKind::Request, json!({"question": "test?"})),
        (MessageKind::Message, json!({"topic": "t", "data": {}})),
        (MessageKind::Response, json!({"answer": "42"})),
        (MessageKind::Error, json!({"code": "fail"})),
    ];

    for (kind, payload) in payloads {
        let env = Envelope::new(a.clone(), b.clone(), kind, payload.clone());
        let encoded = encode(&env).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.kind, kind, "kind mismatch for {kind}");
        assert_eq!(decoded.from, Some(AgentId::from(a.as_str())));
        assert_eq!(decoded.to, Some(AgentId::from(b.as_str())));
    }
}

// =========================================================================
// Transport integration — QUIC peer-to-peer
// =========================================================================

/// Two transports connect via QUIC (TLS handshake authenticates).
#[tokio::test]
async fn transport_connect() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = transport_b.local_addr().unwrap();

    let peer_b = make_peer_record(&id_b, addr_b);
    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();
    assert!(conn.close_reason().is_none());
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}

/// Bidirectional request gets an error response (default_error_response).
#[tokio::test]
async fn transport_request_gets_error_response() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, _transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = _transport_b.local_addr().unwrap();

    let peer_b = make_peer_record(&id_b, addr_b);

    let request = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Request,
        json!({"question": "What is 2+2?"}),
    );

    let result = transport_a.send(&peer_b, request.clone()).await.unwrap();
    let response = result.expect("expected response for request");
    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(response.ref_id, Some(request.id));
}

/// Unidirectional message delivered without response.
#[tokio::test]
async fn transport_message_fire_and_forget() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = transport_b.local_addr().unwrap();
    let mut rx_b = transport_b.subscribe_inbound();

    let peer_b = make_peer_record(&id_b, addr_b);

    let message = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Message,
        json!({"topic": "test.topic", "data": {"key": "value"}}),
    );

    let result = transport_a.send(&peer_b, message).await.unwrap();
    assert!(result.is_none(), "message should not return a response");

    let received = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
        .await
        .expect("timeout")
        .expect("recv");
    assert_eq!(received.kind, MessageKind::Message);
    assert_eq!(received.from, Some(AgentId::from(id_a.agent_id())));
}
