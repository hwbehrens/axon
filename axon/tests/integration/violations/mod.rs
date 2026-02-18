use crate::*;

mod connection;

/// `spec/WIRE_FORMAT.md` validation rules: invalid envelope on bidi request returns error.
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

/// After connection establishment, multiple requests all get error responses.
/// Verifies the connection stays open and handles multiple requests.
#[tokio::test]
async fn violation_connection_survives_multiple_requests() {
    let (id_a, _dir_a) = make_identity();
    let (id_b, _dir_b) = make_identity();

    let (transport_a, _transport_b, _, _) = make_transport_pair(&id_a, &id_b).await;
    let addr_b = _transport_b.local_addr().unwrap();

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
