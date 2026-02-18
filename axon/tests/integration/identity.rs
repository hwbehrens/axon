use crate::*;

// =========================================================================
// Identity integration
// =========================================================================

/// Identity generates, persists, and reloads consistently.
#[test]
fn identity_roundtrip_persistence() {
    let dir = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let id1 = Identity::load_or_generate(&paths).unwrap();
    let id2 = Identity::load_or_generate(&paths).unwrap();

    assert_eq!(id1.agent_id(), id2.agent_id());
    assert_eq!(id1.public_key_base64(), id2.public_key_base64());

    // Certs are ephemeral but should both be valid DER.
    let cert1 = id1.make_quic_certificate().unwrap();
    let cert2 = id2.make_quic_certificate().unwrap();
    assert!(!cert1.cert_der.is_empty());
    assert!(!cert2.cert_der.is_empty());
}

/// Certificate contains Ed25519 public key that matches identity.
#[test]
fn cert_pubkey_matches_identity() {
    let (id, _dir) = make_identity();
    let cert = id.make_quic_certificate().unwrap();

    let extracted = axon::transport::extract_ed25519_pubkey_from_cert_der(&cert.cert_der).unwrap();
    let extracted_b64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, extracted);
    assert_eq!(extracted_b64, id.public_key_base64());
}

// =========================================================================
// Config integration
// =========================================================================

/// Config with static peers round-trips through YAML.
#[tokio::test]
async fn config_static_peers_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    std::fs::write(
        &path,
        r#"
port: 8000
peers:
  - agent_id: "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    addr: "10.0.0.1:7100"
    pubkey: "dGVzdHB1YmtleQ=="
"#,
    )
    .unwrap();
    let config = Config::load(&path).await.unwrap();
    assert_eq!(config.effective_port(None), 8000);
    assert_eq!(config.peers.len(), 1);
    assert_eq!(
        config.peers[0].agent_id,
        "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8"
    );
}

/// Known peers save and load integration.
#[tokio::test]
async fn known_peers_save_load_integration() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("known_peers.json");
    let peers = vec![
        KnownPeer {
            agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "Zm9v".to_string(),
            last_seen_unix_ms: 1000,
        },
        KnownPeer {
            agent_id: "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            addr: "10.0.0.2:7100".parse().unwrap(),
            pubkey: "YmFy".to_string(),
            last_seen_unix_ms: 2000,
        },
    ];

    save_known_peers(&path, &peers).await.unwrap();
    let loaded = load_known_peers(&path).await.unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(
        loaded[0].agent_id,
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        loaded[1].agent_id,
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
}
