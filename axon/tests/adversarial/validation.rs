use crate::*;

// =========================================================================
// §4 Envelope validation edge cases
// =========================================================================

/// Envelope::validate() rejects malformed agent IDs, zero versions, etc.
#[test]
fn envelope_validation_edge_cases() {
    // Uppercase hex — is_ascii_hexdigit accepts uppercase, so validate() passes.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.A1B2C3D4E5F6A7B8A1B2C3D4E5F6A7B8".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_ok(),
        "uppercase hex is valid per is_ascii_hexdigit"
    );

    // Non-hex characters in agent ID.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "non-hex characters should fail validation"
    );

    // Missing ed25519. prefix.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "agent ID without ed25519. prefix should fail validation"
    );

    // Hex part too short (31 hex chars).
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "31-char hex suffix should fail validation"
    );

    // Hex part too long (33 hex chars).
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b80".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "33-char hex suffix should fail validation"
    );

    // Version = 0.
    let env = Envelope {
        v: 0,
        id: uuid::Uuid::new_v4(),
        from: agent_a().into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(env.validate().is_err(), "version 0 should fail validation");

    // Timestamp = 0.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: agent_a().into(),
        to: agent_b().into(),
        ts: 0,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "timestamp 0 should fail validation"
    );

    // Empty from string.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: String::new().into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(env.validate().is_err(), "empty from should fail validation");

    // Empty to string.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: agent_a().into(),
        to: String::new().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(env.validate().is_err(), "empty to should fail validation");

    // Unicode characters in agent IDs.
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4(),
        from: "a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b✓".into(),
        to: agent_b().into(),
        ts: 1_700_000_000_000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    };
    assert!(
        env.validate().is_err(),
        "unicode in agent ID should fail validation"
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
            MessageKind::Query,
            json!({"question": payload_str}),
        );
        match encode(&env) {
            Ok(encoded) => {
                let body_len = encoded.len() - 4;
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
        MessageKind::Query,
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
        MessageKind::Query,
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
    let incomplete = br#"{"v":1,"id":"550e8400-e29b-41d4-a716-446655440000"}"#;
    assert!(
        decode(incomplete).is_err(),
        "JSON missing required fields should return Err"
    );

    // Valid JSON but wrong types (v as string instead of number).
    let wrong_types = br#"{"v":"one","id":"550e8400-e29b-41d4-a716-446655440000","from":"a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8","to":"f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","ts":1700000000000,"kind":"ping","payload":{}}"#;
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
        load_known_peers(&path).is_err(),
        "random bytes should return Err"
    );

    // Truncated JSON.
    std::fs::write(&path, b"[{\"agent_id\":\"aaa").unwrap();
    assert!(
        load_known_peers(&path).is_err(),
        "truncated JSON should return Err"
    );

    // Wrong schema — array of strings instead of KnownPeer objects.
    std::fs::write(&path, b"[\"not\",\"a\",\"peer\"]").unwrap();
    assert!(
        load_known_peers(&path).is_err(),
        "wrong schema should return Err"
    );

    // Empty array — valid, should load as empty vec.
    std::fs::write(&path, b"[]").unwrap();
    let peers = load_known_peers(&path).unwrap();
    assert!(peers.is_empty(), "empty array should load as empty vec");

    // Valid data.
    let valid = vec![KnownPeer {
        agent_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: "10.0.0.1:7100".parse().unwrap(),
        pubkey: "Zm9v".to_string(),
        last_seen_unix_ms: 1000,
    }];
    save_known_peers(&path, &valid).await.unwrap();
    let loaded = load_known_peers(&path).unwrap();
    assert_eq!(loaded.len(), 1, "valid data should load correctly");
    assert_eq!(loaded[0].agent_id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
}

// =========================================================================
// §9 Config corruption resilience
// =========================================================================

/// Config::load handles corrupt, invalid, and missing files gracefully.
#[test]
fn config_corruption_resilience() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    // Random bytes.
    std::fs::write(&path, b"\x80\x81\x82\xff random garbage").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "random bytes should return Err"
    );

    // Invalid TOML syntax.
    std::fs::write(&path, b"[invalid toml =====").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "invalid TOML should return Err"
    );

    // Valid TOML but wrong types (port as string).
    std::fs::write(&path, b"port = \"not a number\"").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "wrong types should return Err"
    );

    // Valid TOML but wrong nested type (peers as string).
    std::fs::write(&path, b"peers = \"not an array\"").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "wrong nested types should return Err"
    );

    // Non-existent path returns default config (not Err).
    let missing = dir.path().join("nonexistent.toml");
    let config = Config::load(&missing).unwrap();
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
    let config = Config::load(&path).unwrap();
    assert_eq!(config.effective_port(None), 9000);
    assert!(config.peers.is_empty());
}
