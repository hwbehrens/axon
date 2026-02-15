use super::*;
use crate::config::AxonPaths;
use crate::identity::Identity;
use std::path::PathBuf;
use tempfile::tempdir;

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

#[test]
fn derive_agent_id_deterministic() {
    let key = [42u8; 32];
    let id1 = derive_agent_id_from_pubkey_bytes(&key);
    let id2 = derive_agent_id_from_pubkey_bytes(&key);
    assert_eq!(id1, id2);
    assert_eq!(id1.len(), 40);
    assert!(id1.starts_with("ed25519."));
}
