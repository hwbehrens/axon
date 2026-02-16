use crate::*;

// =========================================================================
// Connection handler invariant tests (exercise refactored helpers)
// =========================================================================

/// Invariant: notify (fire-and-forget) is delivered via unidirectional stream
/// and the receiver forwards it to inbound subscribers.
/// Exercises: handle_uni_stream (accept fire-and-forget kinds).
#[tokio::test]
async fn connection_uni_notify_delivered() {
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

    // Send notify (unidirectional)
    let notify = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Notify,
        json!({"topic": "connection.test", "data": {"from": "handler"}, "importance": "low"}),
    );
    let notify_id = notify.id;
    let result = transport_a.send(&peer_b, notify).await.unwrap();
    assert!(result.is_none(), "notify must not return a response");

    // Verify delivery
    let received = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
            .await
            .expect("timeout waiting for notify")
            .expect("recv failed");
        if msg.kind == MessageKind::Notify {
            break msg;
        }
    };
    assert_eq!(received.id, notify_id);
    assert_eq!(received.from, id_a.agent_id());
}

/// Invariant: unknown message kind on bidi stream returns error(unknown_kind).
/// Exercises: handle_authenticated_bidi (unknown kind branch) over the wire.
/// Sends raw bytes with a fabricated kind through a real QUIC bidi stream.
#[tokio::test]
async fn connection_bidi_unknown_kind_returns_error() {
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

    // Establish connection (hello completes automatically)
    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();

    // Craft an envelope with a kind the receiver won't recognize
    let env = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Query, // placeholder
        json!({"question": "test"}),
    );
    let mut json_val = serde_json::to_value(&env).unwrap();
    json_val["kind"] = json!("totally_made_up_kind");
    let raw_bytes = serde_json::to_vec(&json_val).unwrap();

    // Send raw bytes over a bidi stream (bypassing typed send API)
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&raw_bytes).await.unwrap();
    send.finish().unwrap();

    // Read the response — should be error(unknown_kind)
    let response_bytes = recv.read_to_end(65536).await.unwrap();
    let response: Envelope = serde_json::from_slice(&response_bytes).unwrap();
    assert_eq!(response.kind, MessageKind::Error);
    let payload = response.payload_value().unwrap();
    assert_eq!(payload["code"], "unknown_kind");
}

/// Invariant: after hello completes, all bidi request types get their
/// expected response kinds. Exercises the full handle_bidi_stream →
/// handle_authenticated_bidi → auto_response pipeline through the wire.
#[tokio::test]
async fn connection_all_bidi_kinds_get_correct_response() {
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
    let from = id_a.agent_id().to_string();
    let to = id_b.agent_id().to_string();

    // Test all bidi request kinds and their expected response kinds
    let cases: Vec<(MessageKind, Value, MessageKind)> = vec![
        (MessageKind::Ping, json!({}), MessageKind::Pong),
        (
            MessageKind::Query,
            json!({"question": "test?", "domain": "meta"}),
            MessageKind::Response,
        ),
        (
            MessageKind::Delegate,
            json!({"task": "do it", "priority": "normal", "report_back": true}),
            MessageKind::Ack,
        ),
        (
            MessageKind::Cancel,
            json!({"reason": "nevermind"}),
            MessageKind::Ack,
        ),
        (MessageKind::Discover, json!({}), MessageKind::Capabilities),
    ];

    for (req_kind, payload, expected_resp_kind) in cases {
        let env = Envelope::new(from.clone(), to.clone(), req_kind, payload);
        let env_id = env.id;
        let result = transport_a.send(&peer_b, env).await.unwrap();
        let resp = result.unwrap_or_else(|| {
            panic!("expected response for {req_kind}");
        });
        assert_eq!(
            resp.kind, expected_resp_kind,
            "{req_kind} should get {expected_resp_kind}, got {}",
            resp.kind
        );
        assert_eq!(
            resp.ref_id,
            Some(env_id),
            "{req_kind} response must ref the request"
        );
    }
}
