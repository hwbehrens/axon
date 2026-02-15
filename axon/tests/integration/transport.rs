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
        (MessageKind::Ping, json!({})),
        (
            MessageKind::Query,
            json!({"question": "test?", "domain": "meta"}),
        ),
        (
            MessageKind::Notify,
            json!({"topic": "t", "data": {}, "importance": "high"}),
        ),
        (
            MessageKind::Delegate,
            json!({"task": "do it", "priority": "urgent", "report_back": true}),
        ),
        (MessageKind::Discover, json!({})),
        (MessageKind::Cancel, json!({"reason": "changed mind"})),
    ];

    for (kind, payload) in payloads {
        let env = Envelope::new(a.clone(), b.clone(), kind, payload.clone());
        let encoded = encode(&env).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.kind, kind, "kind mismatch for {kind}");
        assert_eq!(decoded.from, a);
        assert_eq!(decoded.to, b);
    }
}

// =========================================================================
// Transport integration — QUIC peer-to-peer
// =========================================================================

/// Two transports connect and complete hello exchange.
#[tokio::test]
async fn transport_hello_exchange() {
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
    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();
    assert!(conn.close_reason().is_none());
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}

/// Bidirectional query gets a response.
#[tokio::test]
async fn transport_query_gets_response() {
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

    let query = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Query,
        json!({"question": "What is 2+2?", "domain": "math"}),
    );

    let result = transport_a.send(&peer_b, query.clone()).await.unwrap();
    let response = result.expect("expected response for query");
    assert_eq!(response.kind, MessageKind::Response);
    assert_eq!(response.ref_id, Some(query.id));
}

/// Bidirectional discover gets capabilities.
#[tokio::test]
async fn transport_discover_gets_capabilities() {
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

    let discover = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Discover,
        json!({}),
    );

    let result = transport_a.send(&peer_b, discover.clone()).await.unwrap();
    let response = result.expect("expected response for discover");
    assert_eq!(response.kind, MessageKind::Capabilities);
    assert_eq!(response.ref_id, Some(discover.id));
}

/// Bidirectional delegate gets ack.
#[tokio::test]
async fn transport_delegate_gets_ack() {
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

    let delegate = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Delegate,
        json!({"task": "do something", "priority": "normal", "report_back": true}),
    );

    let result = transport_a.send(&peer_b, delegate.clone()).await.unwrap();
    let response = result.expect("expected ack for delegate");
    assert_eq!(response.kind, MessageKind::Ack);
    assert_eq!(response.ref_id, Some(delegate.id));
}

/// Cancel gets ack.
#[tokio::test]
async fn transport_cancel_gets_ack() {
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

    let cancel = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Cancel,
        json!({"reason": "plans changed"}),
    );

    let result = transport_a.send(&peer_b, cancel.clone()).await.unwrap();
    let response = result.expect("expected ack for cancel");
    assert_eq!(response.kind, MessageKind::Ack);
    assert_eq!(response.ref_id, Some(cancel.id));
}

/// Unidirectional notify delivered without response.
#[tokio::test]
async fn transport_notify_fire_and_forget() {
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
        json!({"topic": "test.topic", "data": {"key": "value"}, "importance": "low"}),
    );

    let result = transport_a.send(&peer_b, notify).await.unwrap();
    assert!(result.is_none(), "notify should not return a response");

    // Drain until we find the notify (hello is also broadcast).
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
    assert_eq!(received.from, id_a.agent_id());
}
