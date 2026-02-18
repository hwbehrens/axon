use crate::*;

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
