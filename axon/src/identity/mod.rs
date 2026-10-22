use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};

use crate::config::AxonPaths;
use crate::message::AgentId;

#[derive(Debug, Clone)]
pub struct Identity {
    signing_key: SigningKey,
    agent_id: String,
    public_key_base64: String,
}

// Deliberately not `Debug`: `key_der` holds the PKCS#8 private key and must
// never be formatted into a log line or panic message.
#[derive(Clone)]
pub struct QuicCertificate {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl Identity {
    pub fn load_or_generate(paths: &AxonPaths) -> Result<Self> {
        paths.ensure_root_exists()?;

        let signing_key = match load_existing_signing_key(&paths.identity_key)? {
            Some(key) => key,
            None => {
                let mut seed = [0u8; 32];
                getrandom::fill(&mut seed)
                    .map_err(|err| anyhow!("failed to gather randomness: {err}"))?;
                let key = SigningKey::from_bytes(&seed);
                write_seed_as_base64(&paths.identity_key, &seed)?;
                key
            }
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

/// Hard cap on the encoded identity key. The canonical encoding is 44
/// base64 characters; the cap leaves generous headroom for surrounding
/// whitespace while refusing unbounded reads from a hostile or corrupted
/// file before any decoding happens.
const MAX_IDENTITY_KEY_BYTES: usize = 4096;

/// Load the existing signing key, or `None` when `identity.key` is absent
/// (first run, which generates and persists a new key).
///
/// The path itself is inspected with `symlink_metadata`, so symlinks, FIFOs,
/// devices, and directories are refused rather than followed or opened (an
/// `open` on a FIFO with no writer would block startup forever). The encoded
/// key is read through a bounded `take`, so oversized input is rejected
/// before base64 decoding allocates anything proportional to it.
fn load_existing_signing_key(path: &Path) -> Result<Option<SigningKey>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    if !meta.file_type().is_file() {
        anyhow::bail!(
            "refusing to load non-regular identity.key file: {}. \
             Remove identity.key and identity.pub from this root to re-initialize identity.",
            path.display()
        );
    }
    enforce_private_key_permissions(path)?;

    let file = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut raw = Vec::new();
    file.take(MAX_IDENTITY_KEY_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.len() > MAX_IDENTITY_KEY_BYTES {
        anyhow::bail!(
            "identity.key at {} exceeds the {MAX_IDENTITY_KEY_BYTES}-byte maximum; \
             refusing to decode. Remove identity.key and identity.pub from this root to re-initialize identity.",
            path.display()
        );
    }

    let text = std::str::from_utf8(&raw).map_err(|_| {
        anyhow!(
            "invalid identity.key format at {}: expected base64 text containing a 32-byte seed; \
             non-text key data is unsupported. \
             Remove identity.key and identity.pub from this root to re-initialize identity.",
            path.display()
        )
    })?;
    let seed = decode_seed_from_base64_text(text, path)?;
    Ok(Some(SigningKey::from_bytes(&seed)))
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
    let mut file = create_private_key_file(path)
        .with_context(|| format!("failed to write private key: {}", path.display()))?;
    file.write_all(key_b64.as_bytes())
        .with_context(|| format!("failed to write private key: {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync private key: {}", path.display()))?;
    Ok(())
}

/// Create the private key file with mode 0600 applied at `open(2)` time and
/// `O_EXCL` semantics. Writing the seed first and chmod-ing afterwards (the
/// previous shape) left the seed readable at 0644 for a window under a
/// permissive umask; `create_new` also refuses to clobber an existing key.
fn create_private_key_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

fn enforce_private_key_permissions(path: &Path) -> Result<()> {
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
    AgentId::from_pubkey_bytes(&verifying_key.to_bytes())
        .expect("Ed25519 verifying keys are exactly 32 bytes")
        .to_string()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
