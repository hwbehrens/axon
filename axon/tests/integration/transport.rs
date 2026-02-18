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
