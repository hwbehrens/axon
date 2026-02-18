use super::*;
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::peer_table::PeerTable;
use rustls::SignatureScheme;
use rustls::client::danger::ServerCertVerifier;
use rustls::server::danger::ClientCertVerifier;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock as StdRwLock};
use tempfile::tempdir;
use tokio::sync::broadcast;

#[test]
fn cert_pubkey_extraction_matches_identity() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");

    let extracted = extract_ed25519_pubkey_from_cert_der(&cert.cert_der).expect("extract pubkey");
    let cert_pubkey_b64 = STANDARD.encode(extracted);

    assert_eq!(cert_pubkey_b64, identity.public_key_base64());
    assert_eq!(
        derive_agent_id_from_pubkey_bytes(&extracted),
        identity.agent_id()
    );
}

fn make_test_verifier() -> PeerCertVerifier {
    let (pair_request_tx, _) = broadcast::channel(8);
    PeerCertVerifier {
        expected_pubkeys: PeerTable::new().pubkey_map(),
        pair_request_tx,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    }
}

fn make_test_client_verifier() -> PeerClientCertVerifier {
    let (pair_request_tx, _) = broadcast::channel(8);
    PeerClientCertVerifier {
        expected_pubkeys: PeerTable::new().pubkey_map(),
        roots: vec![],
        pair_request_tx,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    }
}

#[test]
fn verifier_supported_schemes_include_ed25519() {
    ensure_crypto_provider();
    let server_verifier = make_test_verifier();
    let client_verifier = make_test_client_verifier();

    let server_schemes = server_verifier.supported_verify_schemes();
    let client_schemes = client_verifier.supported_verify_schemes();

    assert!(
        server_schemes.contains(&SignatureScheme::ED25519),
        "server verifier must support Ed25519"
    );
    assert!(
        client_schemes.contains(&SignatureScheme::ED25519),
        "client verifier must support Ed25519"
    );
    // Both should return the same schemes (delegating to the same provider)
    assert_eq!(server_schemes, client_schemes);
}

#[test]
fn verifier_supported_schemes_match_ring_provider() {
    ensure_crypto_provider();
    let ring_schemes = rustls::crypto::ring::default_provider()
        .signature_verification_algorithms
        .supported_schemes();

    let server_verifier = make_test_verifier();
    let client_verifier = make_test_client_verifier();

    assert_eq!(
        server_verifier.supported_verify_schemes(),
        ring_schemes,
        "server verifier schemes must match ring provider exactly"
    );
    assert_eq!(
        client_verifier.supported_verify_schemes(),
        ring_schemes,
        "client verifier schemes must match ring provider exactly"
    );
}

#[test]
fn server_verifier_rejects_unknown_peer() {
    ensure_crypto_provider();
    let verifier = make_test_verifier();
    // empty expected_pubkeys → any peer should be rejected
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");
    let cert_der = CertificateDer::from(cert.cert_der);
    let agent_id_string = identity.agent_id().to_string();
    let server_name = ServerName::try_from(agent_id_string.as_str()).unwrap();

    let result = verifier.verify_server_cert(
        &cert_der,
        &[],
        &server_name,
        &[],
        rustls::pki_types::UnixTime::now(),
    );
    assert!(result.is_err(), "unknown peer must be rejected");
    assert!(
        format!("{}", result.unwrap_err()).contains("no public key on record"),
        "error should mention missing discovery data"
    );
}

#[test]
fn server_verifier_accepts_known_peer() {
    ensure_crypto_provider();
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");

    let pubkey_map = Arc::new(StdRwLock::new(HashMap::new()));
    pubkey_map.write().unwrap().insert(
        identity.agent_id().to_string(),
        identity.public_key_base64().to_string(),
    );

    let verifier = PeerCertVerifier {
        expected_pubkeys: pubkey_map,
        pair_request_tx: broadcast::channel(8).0,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    };
    let cert_der = CertificateDer::from(cert.cert_der);
    let agent_id_string = identity.agent_id().to_string();
    let server_name = ServerName::try_from(agent_id_string.as_str()).unwrap();

    let result = verifier.verify_server_cert(
        &cert_der,
        &[],
        &server_name,
        &[],
        rustls::pki_types::UnixTime::now(),
    );
    assert!(
        result.is_ok(),
        "known peer with matching key must be accepted"
    );
}

#[test]
fn server_verifier_accepts_uppercase_expected_agent_id() {
    ensure_crypto_provider();
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");

    let uppercase_id = identity.agent_id().to_ascii_uppercase();
    let pubkey_map = Arc::new(StdRwLock::new(HashMap::new()));
    pubkey_map.write().unwrap().insert(
        uppercase_id.clone(),
        identity.public_key_base64().to_string(),
    );

    let verifier = PeerCertVerifier {
        expected_pubkeys: pubkey_map,
        pair_request_tx: broadcast::channel(8).0,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    };
    let cert_der = CertificateDer::from(cert.cert_der);
    let server_name = ServerName::try_from(uppercase_id.as_str()).unwrap();

    let result = verifier.verify_server_cert(
        &cert_der,
        &[],
        &server_name,
        &[],
        rustls::pki_types::UnixTime::now(),
    );
    assert!(
        result.is_ok(),
        "server verifier should accept uppercase expected agent id"
    );
}

#[test]
fn server_verifier_rejects_pubkey_mismatch() {
    ensure_crypto_provider();
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");

    let pubkey_map = Arc::new(StdRwLock::new(HashMap::new()));
    // Register the agent_id but with a wrong pubkey
    pubkey_map
        .write()
        .unwrap()
        .insert(identity.agent_id().to_string(), STANDARD.encode([99u8; 32]));

    let verifier = PeerCertVerifier {
        expected_pubkeys: pubkey_map,
        pair_request_tx: broadcast::channel(8).0,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    };
    let cert_der = CertificateDer::from(cert.cert_der);
    let agent_id_string = identity.agent_id().to_string();
    let server_name = ServerName::try_from(agent_id_string.as_str()).unwrap();

    let result = verifier.verify_server_cert(
        &cert_der,
        &[],
        &server_name,
        &[],
        rustls::pki_types::UnixTime::now(),
    );
    assert!(result.is_err(), "mismatched pubkey must be rejected");
    assert!(
        format!("{}", result.unwrap_err()).contains("mismatch"),
        "error should mention key mismatch"
    );
}

#[test]
fn client_verifier_rejects_unknown_peer() {
    ensure_crypto_provider();
    let verifier = make_test_client_verifier();
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");
    let cert_der = CertificateDer::from(cert.cert_der);

    let result = verifier.verify_client_cert(&cert_der, &[], rustls::pki_types::UnixTime::now());
    assert!(result.is_err(), "unknown client must be rejected");
}

#[test]
fn client_verifier_accepts_uppercase_peer_table_key() {
    ensure_crypto_provider();
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");

    let uppercase_id = identity.agent_id().to_ascii_uppercase();
    let pubkey_map = Arc::new(StdRwLock::new(HashMap::new()));
    pubkey_map
        .write()
        .unwrap()
        .insert(uppercase_id, identity.public_key_base64().to_string());

    let verifier = PeerClientCertVerifier {
        expected_pubkeys: pubkey_map,
        roots: vec![],
        pair_request_tx: broadcast::channel(8).0,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    };
    let cert_der = CertificateDer::from(cert.cert_der);

    let result = verifier.verify_client_cert(&cert_der, &[], rustls::pki_types::UnixTime::now());
    assert!(
        result.is_ok(),
        "client verifier should accept uppercase peer-table key"
    );
}

#[test]
fn derive_agent_id_deterministic() {
    let key = [42u8; 32];
    let id1 = derive_agent_id_from_pubkey_bytes(&key);
    let id2 = derive_agent_id_from_pubkey_bytes(&key);
    assert_eq!(id1, id2);
    assert_eq!(id1.len(), 40);
    assert!(id1.starts_with("ed25519."));
}

#[test]
fn unknown_peer_pair_request_is_rate_limited() {
    ensure_crypto_provider();
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");
    let cert_der = CertificateDer::from(cert.cert_der);
    let agent_id_string = identity.agent_id().to_string();
    let server_name = ServerName::try_from(agent_id_string.as_str()).unwrap();

    let (pair_request_tx, mut pair_request_rx) = broadcast::channel(8);
    let verifier = PeerCertVerifier {
        expected_pubkeys: PeerTable::new().pubkey_map(),
        pair_request_tx,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    };

    let _ = verifier.verify_server_cert(
        &cert_der,
        &[],
        &server_name,
        &[],
        rustls::pki_types::UnixTime::now(),
    );
    let _ = verifier.verify_server_cert(
        &cert_der,
        &[],
        &server_name,
        &[],
        rustls::pki_types::UnixTime::now(),
    );

    let first = pair_request_rx.try_recv().expect("first pair_request");
    assert_eq!(first.agent_id, agent_id_string);
    assert!(pair_request_rx.try_recv().is_err());
}

#[tokio::test]
async fn unknown_peer_pair_request_includes_remote_addr_from_handshake_context() {
    ensure_crypto_provider();
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let cert = identity.make_quic_certificate().expect("cert");
    let cert_der = CertificateDer::from(cert.cert_der);
    let agent_id_string = identity.agent_id().to_string();
    let server_name = ServerName::try_from(agent_id_string.as_str()).unwrap();

    let (pair_request_tx, mut pair_request_rx) = broadcast::channel(8);
    let verifier = PeerCertVerifier {
        expected_pubkeys: PeerTable::new().pubkey_map(),
        pair_request_tx,
        pair_request_seen: Arc::new(Mutex::new(HashMap::new())),
    };

    let remote_addr: SocketAddr = "127.0.0.1:7444".parse().unwrap();
    let _ = with_handshake_remote_addr(remote_addr, async {
        verifier.verify_server_cert(
            &cert_der,
            &[],
            &server_name,
            &[],
            rustls::pki_types::UnixTime::now(),
        )
    })
    .await;

    let first = pair_request_rx.try_recv().expect("first pair_request");
    assert_eq!(first.agent_id, agent_id_string);
    assert_eq!(first.addr.as_deref(), Some("127.0.0.1:7444"));
}
