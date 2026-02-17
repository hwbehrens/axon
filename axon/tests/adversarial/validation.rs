use crate::*;

// =========================================================================
// §4 Envelope validation edge cases
// =========================================================================

/// Envelope::validate() rejects nil UUIDs. Agent-ID format, version, and
/// timestamp checks were removed in the architecture simplification.
#[test]
fn envelope_validation_edge_cases() {
    // Valid envelope — validate() should pass.
    let env = Envelope {
        id: uuid::Uuid::new_v4(),
        kind: MessageKind::Request,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
        from: Some("ed25519.A1B2C3D4E5F6A7B8A1B2C3D4E5F6A7B8".into()),
        to: Some(agent_b().into()),
    };
    assert!(env.validate().is_ok(), "valid envelope should pass");

    // Nil UUID — validate() should reject.
    let env = Envelope {
        id: uuid::Uuid::nil(),
        kind: MessageKind::Request,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
        from: Some(agent_a().into()),
        to: Some(agent_b().into()),
    };
    assert!(env.validate().is_err(), "nil UUID should fail validation");

    // None from/to — valid in simplified model (daemon fills these in).
    let env = Envelope {
        id: uuid::Uuid::new_v4(),
        kind: MessageKind::Message,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
        from: None,
        to: None,
    };
    assert!(
        env.validate().is_ok(),
        "None from/to should pass validation in simplified model"
    );
}

// =========================================================================
// §5 Wire format boundary conditions
// =========================================================================

/// Wire encode/decode with boundary sizes, truncated data, and wrong types.
#[test]
fn wire_format_boundary_conditions() {
    // Find a payload size that produces an envelope exactly at MAX_MESSAGE_SIZE.
    // We probe by binary-searching the payload string length.
    let mut low = 0usize;
    let mut high = MAX_MESSAGE_SIZE as usize;
    while low + 1 < high {
        let mid = (low + high) / 2;
        let payload_str = "x".repeat(mid);
        let env = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Request,
            json!({"question": payload_str}),
        );
        match encode(&env) {
            Ok(encoded) => {
                let body_len = encoded.len();
                if body_len <= MAX_MESSAGE_SIZE as usize {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            Err(_) => {
                high = mid;
            }
        }
    }

    // `low` is the largest payload string that fits. Verify it encodes.
    let payload_str = "x".repeat(low);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"question": payload_str}),
    );
    assert!(
        encode(&env).is_ok(),
        "envelope at MAX_MESSAGE_SIZE boundary should encode"
    );

    // One more char should fail.
    let payload_str_over = "x".repeat(low + 1);
    let env_over = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"question": payload_str_over}),
    );
    assert!(
        encode(&env_over).is_err(),
        "envelope exceeding MAX_MESSAGE_SIZE should fail"
    );

    // Decode with empty bytes.
    assert!(
        decode(b"").is_err(),
        "decoding empty bytes should return Err"
    );

    // Decode with single byte.
    assert!(
        decode(b"{").is_err(),
        "decoding single byte should return Err"
    );

    // Decode with 3 bytes.
    assert!(
        decode(b"abc").is_err(),
        "decoding 3 bytes should return Err"
    );

    // Valid JSON but missing required fields.
    let incomplete = br#"{"id":"550e8400-e29b-41d4-a716-446655440000"}"#;
    assert!(
        decode(incomplete).is_err(),
        "JSON missing required fields should return Err"
    );

    // Valid JSON but wrong types (kind as number instead of string).
    let wrong_types = br#"{"id":"550e8400-e29b-41d4-a716-446655440000","kind":123,"payload":{}}"#;
    assert!(
        decode(wrong_types).is_err(),
        "JSON with wrong types should return Err"
    );
}

// =========================================================================
// §8 Known peers corruption resilience
// =========================================================================

/// load_known_peers handles corrupt, truncated, and wrong-schema files
/// without panicking.
#[tokio::test]
async fn known_peers_corruption_resilience() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("known_peers.json");

    // Random bytes.
    std::fs::write(&path, b"\x80\x81\x82\xff random garbage").unwrap();
    assert!(
        load_known_peers(&path).await.is_err(),
        "random bytes should return Err"
    );

    // Truncated JSON.
    std::fs::write(&path, b"[{\"agent_id\":\"aaa").unwrap();
    assert!(
        load_known_peers(&path).await.is_err(),
        "truncated JSON should return Err"
    );

    // Wrong schema — array of strings instead of KnownPeer objects.
    std::fs::write(&path, b"[\"not\",\"a\",\"peer\"]").unwrap();
    assert!(
        load_known_peers(&path).await.is_err(),
        "wrong schema should return Err"
    );

    // Empty array — valid, should load as empty vec.
    std::fs::write(&path, b"[]").unwrap();
    let peers = load_known_peers(&path).await.unwrap();
    assert!(peers.is_empty(), "empty array should load as empty vec");

    // Valid data.
    let valid = vec![KnownPeer {
        agent_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: "10.0.0.1:7100".parse().unwrap(),
        pubkey: "Zm9v".to_string(),
        last_seen_unix_ms: 1000,
    }];
    save_known_peers(&path, &valid).await.unwrap();
    let loaded = load_known_peers(&path).await.unwrap();
    assert_eq!(loaded.len(), 1, "valid data should load correctly");
    assert_eq!(loaded[0].agent_id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
}

// =========================================================================
// §9 Config corruption resilience
// =========================================================================

/// Config::load handles corrupt, invalid, and missing files gracefully.
#[tokio::test]
async fn config_corruption_resilience() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    // Random bytes.
    std::fs::write(&path, b"\x80\x81\x82\xff random garbage").unwrap();
    assert!(
        Config::load(&path).await.is_err(),
        "random bytes should return Err"
    );

    // Invalid TOML syntax.
    std::fs::write(&path, b"[invalid toml =====").unwrap();
    assert!(
        Config::load(&path).await.is_err(),
        "invalid TOML should return Err"
    );

    // Valid TOML but wrong types (port as string).
    std::fs::write(&path, b"port = \"not a number\"").unwrap();
    assert!(
        Config::load(&path).await.is_err(),
        "wrong types should return Err"
    );

    // Valid TOML but wrong nested type (peers as string).
    std::fs::write(&path, b"peers = \"not an array\"").unwrap();
    assert!(
        Config::load(&path).await.is_err(),
        "wrong nested types should return Err"
    );

    // Non-existent path returns default config (not Err).
    let missing = dir.path().join("nonexistent.toml");
    let config = Config::load(&missing).await.unwrap();
    assert_eq!(
        config.effective_port(None),
        7100,
        "missing config should use default port"
    );
    assert!(
        config.peers.is_empty(),
        "missing config should have no peers"
    );

    // Valid minimal config.
    std::fs::write(&path, b"port = 9000").unwrap();
    let config = Config::load(&path).await.unwrap();
    assert_eq!(config.effective_port(None), 9000);
    assert!(config.peers.is_empty());
}
