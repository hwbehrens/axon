use crate::*;

// =========================================================================
// Connection handler invariant tests (exercise refactored helpers)
// =========================================================================

/// Invariant: Message (fire-and-forget) is delivered via unidirectional stream
/// and the receiver forwards it to inbound subscribers.
/// Exercises: handle_uni_stream (accept fire-and-forget kinds).
#[tokio::test]
async fn connection_uni_message_delivered() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = transport_b.local_addr().unwrap();
    let mut rx_b = transport_b.subscribe_inbound();

    let peer_b = make_peer_record(&id_b, addr_b);

    // Send message (unidirectional)
    let message = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Message,
        json!({"topic": "connection.test", "data": {"from": "handler"}}),
    );
    let message_id = message.id;
    let result = transport_a.send(&peer_b, message).await.unwrap();
    assert!(result.is_none(), "message must not return a response");

    // Verify delivery
    let received = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
            .await
            .expect("timeout waiting for message")
            .expect("recv failed");
        if msg.kind == MessageKind::Message {
            break msg;
        }
    };
    assert_eq!(received.id, message_id);
    assert_eq!(received.from, Some(AgentId::from(id_a.agent_id())));
}

/// Invariant: receiver identity must come from TLS-authenticated peer,
/// not wire-provided `from`/`to` fields.
#[tokio::test]
async fn connection_uni_spoofed_from_overwritten_by_tls_identity() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = transport_b.local_addr().unwrap();
    let mut rx_b = transport_b.subscribe_inbound();
    let peer_b = make_peer_record(&id_b, addr_b);

    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();
    let spoofed = Envelope::new(
        "ed25519.ffffffffffffffffffffffffffffffff".to_string(),
        "ed25519.11111111111111111111111111111111".to_string(),
        MessageKind::Message,
        json!({"topic": "connection.spoof", "data": {"x": 1}}),
    );
    let spoofed_id = spoofed.id;
    let raw_bytes = serde_json::to_vec(&spoofed).unwrap();

    let mut send = conn.open_uni().await.unwrap();
    send.write_all(&raw_bytes).await.unwrap();
    send.finish().unwrap();

    let received = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
            .await
            .expect("timeout waiting for spoofed message")
            .expect("recv failed");
        if msg.id == spoofed_id {
            break msg;
        }
    };

    assert_eq!(received.from, Some(AgentId::from(id_a.agent_id())));
    assert_eq!(received.to, Some(AgentId::from(id_b.agent_id())));
}

/// Invariant: unknown message kind on bidi stream returns error.
/// Exercises: handle_authenticated_bidi (unknown kind branch) over the wire.
/// Sends raw bytes with a fabricated kind through a real QUIC bidi stream.
#[tokio::test]
async fn connection_bidi_unknown_kind_returns_error() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, _transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = _transport_b.local_addr().unwrap();

    let peer_b = make_peer_record(&id_b, addr_b);

    // Establish connection (QUIC + mTLS complete automatically)
    let conn = transport_a.ensure_connection(&peer_b).await.unwrap();

    // Craft an envelope with a kind the receiver won't recognize
    let env = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Request, // placeholder
        json!({"data": "test"}),
    );
    let mut json_val = serde_json::to_value(&env).unwrap();
    json_val["kind"] = json!("totally_made_up_kind");
    let raw_bytes = serde_json::to_vec(&json_val).unwrap();

    // Send raw bytes over a bidi stream (bypassing typed send API)
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&raw_bytes).await.unwrap();
    send.finish().unwrap();

    // Read the response — should be error
    let response_bytes = recv.read_to_end(65536).await.unwrap();
    let response: Envelope = serde_json::from_slice(&response_bytes).unwrap();
    assert_eq!(response.kind, MessageKind::Error);
}

/// Invariant: after connection establishment, bidi requests get error responses
/// (default_error_response). Exercises the full handle_bidi_stream →
/// handle_authenticated_bidi → default_error_response pipeline through the wire.
#[tokio::test]
async fn connection_bidi_request_gets_error_response() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, _transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = _transport_b.local_addr().unwrap();

    let peer_b = make_peer_record(&id_b, addr_b);
    let from = id_a.agent_id().to_string();
    let to = id_b.agent_id().to_string();

    let env = Envelope::new(from, to, MessageKind::Request, json!({"data": "test"}));
    let env_id = env.id;
    let result = transport_a.send(&peer_b, env).await.unwrap();
    let resp = result.expect("expected error response for request");
    assert_eq!(resp.kind, MessageKind::Error);
    assert_eq!(resp.ref_id, Some(env_id));
}
