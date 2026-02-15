use std::fs;
use std::os::unix::fs::PermissionsExt;

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use sha2::{Digest, Sha256};

use crate::config::AxonPaths;

#[derive(Debug, Clone)]
pub struct Identity {
    signing_key: SigningKey,
    agent_id: String,
    public_key_base64: String,
}

#[derive(Debug, Clone)]
pub struct QuicCertificate {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl Identity {
    pub fn load_or_generate(paths: &AxonPaths) -> Result<Self> {
        paths.ensure_root_exists()?;

        let signing_key = if paths.identity_key.exists() {
            let raw = fs::read_to_string(&paths.identity_key)
                .with_context(|| format!("failed to read {}", paths.identity_key.display()))?;
            let bytes = STANDARD
                .decode(raw.trim())
                .context("failed to decode identity.key")?;
            let seed: [u8; 32] = bytes
                .try_into()
                .map_err(|v: Vec<u8>| anyhow!("identity.key must contain 32 bytes, got {}", v.len()))?;
            SigningKey::from_bytes(&seed)
        } else {
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).context("failed to gather randomness")?;
            let key = SigningKey::from_bytes(&seed);
            let key_b64 = STANDARD.encode(seed);
            fs::write(&paths.identity_key, &key_b64).with_context(|| {
                format!("failed to write private key: {}", paths.identity_key.display())
            })?;
            fs::set_permissions(&paths.identity_key, fs::Permissions::from_mode(0o600))
                .with_context(|| {
                    format!("failed to set key permissions: {}", paths.identity_key.display())
                })?;
            key
        };

        let verifying = signing_key.verifying_key();
        let pubkey_b64 = STANDARD.encode(verifying.to_bytes());
        fs::write(&paths.identity_pub, &pubkey_b64).with_context(|| {
            format!("failed to write public key: {}", paths.identity_pub.display())
        })?;

        let agent_id = derive_agent_id(&verifying);

        Ok(Self {
            signing_key,
            agent_id,
            public_key_base64: pubkey_b64,
        })
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn public_key_base64(&self) -> &str {
        &self.public_key_base64
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn make_quic_certificate(&self) -> Result<QuicCertificate> {
        let seed = self.signing_key.to_bytes();
        let public_key = self.signing_key.verifying_key().to_bytes();
        let pkcs8 = ed25519_pkcs8_v2(&seed, &public_key);
        let key_pair = KeyPair::from_der_and_sign_algo(&pkcs8, &PKCS_ED25519)
            .context("failed to build rcgen key pair")?;

        let mut params = CertificateParams::new(vec!["localhost".to_string()]);
        params.alg = &PKCS_ED25519;
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, format!("axon-{}", self.agent_id));
        params.key_pair = Some(key_pair);

        let cert = rcgen::Certificate::from_params(params)
            .context("failed to build certificate")?;
        let cert_der = cert.serialize_der().context("failed to serialize certificate")?;
        let key_der = cert.serialize_private_key_der();

        Ok(QuicCertificate { cert_der, key_der })
    }
}

/// PKCS#8 v2 (RFC 8410) wrapping of Ed25519 seed + public key.
/// Needed so rcgen/x509-parser can extract the public key from the DER.
fn ed25519_pkcs8_v2(seed: &[u8; 32], public_key: &[u8; 32]) -> Vec<u8> {
    let mut der = Vec::with_capacity(85);
    der.extend_from_slice(&[
        0x30, 0x53, // SEQUENCE, 83 bytes
        0x02, 0x01, 0x01, // INTEGER 1
        0x30, 0x05, // SEQUENCE, 5 bytes
        0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112 (Ed25519)
        0x04, 0x22, // OCTET STRING, 34 bytes
        0x04, 0x20, // OCTET STRING, 32 bytes (the seed)
    ]);
    der.extend_from_slice(seed);
    der.extend_from_slice(&[
        0xa1, 0x23, // [1] EXPLICIT, 35 bytes
        0x03, 0x21, 0x00, // BIT STRING, 33 bytes (0 unused bits)
    ]);
    der.extend_from_slice(public_key);
    der
}

pub fn derive_agent_id(verifying_key: &VerifyingKey) -> String {
    let digest = Sha256::digest(verifying_key.to_bytes());
    digest[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AxonPaths;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn derive_agent_id_is_32_hex_chars() {
        let mut seed = [7u8; 32];
        let key = SigningKey::from_bytes(&seed);
        let id = derive_agent_id(&key.verifying_key());
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));

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

        let mode = fs::metadata(&paths.identity_key).unwrap().permissions().mode();
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
        assert!(msg.contains("32 bytes"));
    }
}
