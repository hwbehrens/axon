use super::*;
use crate::config::AxonPaths;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn derive_agent_id_is_40_chars_with_prefix() {
    let mut seed = [7u8; 32];
    let key = SigningKey::from_bytes(&seed);
    let id = derive_agent_id(&key.verifying_key());
    assert_eq!(id.len(), 40);
    assert!(id.starts_with("ed25519."));
    assert!(id[8..].chars().all(|c| c.is_ascii_hexdigit()));

    seed[0] = 8;
    let other = SigningKey::from_bytes(&seed);
    assert_ne!(id, derive_agent_id(&other.verifying_key()));
}

#[test]
fn derive_agent_id_is_deterministic() {
    let key = SigningKey::from_bytes(&[42u8; 32]);
    let id1 = derive_agent_id(&key.verifying_key());
    let id2 = derive_agent_id(&key.verifying_key());
    assert_eq!(id1, id2);
}

#[test]
fn load_or_generate_roundtrip_persists_identity() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));

    let first = Identity::load_or_generate(&paths).expect("first load");
    let second = Identity::load_or_generate(&paths).expect("second load");

    assert_eq!(first.agent_id(), second.agent_id());
    assert_eq!(first.public_key_base64(), second.public_key_base64());
}

#[test]
fn private_key_file_permissions() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    Identity::load_or_generate(&paths).expect("generate");

    let mode = fs::metadata(&paths.identity_key)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn public_key_file_is_valid_base64() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("generate");

    let pub_b64 = fs::read_to_string(&paths.identity_pub).expect("read pub");
    let decoded = STANDARD.decode(pub_b64.trim()).expect("decode");
    assert_eq!(decoded.len(), 32);
    assert_eq!(decoded, identity.verifying_key().to_bytes());
}

#[test]
fn cert_generation_produces_material() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("generate");
    let cert = identity.make_quic_certificate().expect("cert");

    assert!(!cert.cert_der.is_empty());
    assert!(!cert.key_der.is_empty());
}

#[test]
fn invalid_key_length_is_rejected() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");

    let bad_b64 = STANDARD.encode([1u8; 16]);
    fs::write(&paths.identity_key, &bad_b64).expect("write bad key");

    let result = Identity::load_or_generate(&paths);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("expected 32 decoded bytes"));
}

#[test]
fn legacy_raw_key_is_rejected() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");

    let raw_seed = [7u8; 32];
    fs::write(&paths.identity_key, raw_seed).expect("write raw key");

    let result = Identity::load_or_generate(&paths);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("invalid identity.key"));
}
