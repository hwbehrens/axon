use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

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
            let raw = fs::read(&paths.identity_key)
                .with_context(|| format!("failed to read {}", paths.identity_key.display()))?;
            let text = std::str::from_utf8(&raw).map_err(|_| {
                anyhow!(
                    "invalid identity.key format at {}: expected base64 text containing a 32-byte seed; \
                     non-text key data is unsupported. \
                     Remove identity.key and identity.pub from this root to re-initialize identity.",
                    paths.identity_key.display()
                )
            })?;
            let seed = decode_seed_from_base64_text(text, &paths.identity_key)?;
            SigningKey::from_bytes(&seed)
        } else {
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed)
                .map_err(|err| anyhow!("failed to gather randomness: {err}"))?;
            let key = SigningKey::from_bytes(&seed);
            write_seed_as_base64(&paths.identity_key, &seed)?;
            key
        };

        let verifying = signing_key.verifying_key();
        let pubkey_b64 = STANDARD.encode(verifying.to_bytes());
        fs::write(&paths.identity_pub, &pubkey_b64).with_context(|| {
            format!(
                "failed to write public key: {}",
                paths.identity_pub.display()
            )
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

        let private_key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(pkcs8.clone()),
        );
        let key_pair = KeyPair::from_der_and_sign_algo(&private_key_der, &PKCS_ED25519)
            .context("failed to build rcgen key pair")?;

        let mut params = CertificateParams::new(vec!["localhost".to_string()])
            .context("failed to create certificate params")?;
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, format!("axon-{}", self.agent_id));

        let cert = params
            .self_signed(&key_pair)
            .context("failed to build self-signed certificate")?;
        let cert_der = cert.der().to_vec();
        let key_der = pkcs8;

        Ok(QuicCertificate { cert_der, key_der })
    }
}

fn decode_seed_from_base64_text(text: &str, path: &Path) -> Result<[u8; 32]> {
    let bytes = STANDARD.decode(text.trim()).map_err(|err| {
        anyhow!(
            "invalid identity.key contents at {}: expected base64 text containing a 32-byte seed ({err}). \
             Remove identity.key and identity.pub from this root to re-initialize identity.",
            path.display()
        )
    })?;
    let seed: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
        anyhow!(
            "invalid identity.key length at {}: expected 32 decoded bytes, got {}. \
             Remove identity.key and identity.pub from this root to re-initialize identity.",
            path.display(),
            v.len()
        )
    })?;
    Ok(seed)
}

fn write_seed_as_base64(path: &Path, seed: &[u8; 32]) -> Result<()> {
    let key_b64 = STANDARD.encode(seed);
    fs::write(path, &key_b64)
        .with_context(|| format!("failed to write private key: {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set key permissions: {}", path.display()))?;
    Ok(())
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
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("ed25519.{hex}")
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
