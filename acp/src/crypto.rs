use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce,
};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use x25519_dalek::{PublicKey, StaticSecret};

/// ACP identity: a static X25519 keypair.
pub struct Identity {
    pub secret: StaticSecret,
    pub public: PublicKey,
}

impl Identity {
    /// Load or generate identity from `~/.acp/`.
    pub fn load_or_generate() -> Result<Self> {
        let dir = Self::acp_dir()?;
        fs::create_dir_all(&dir)?;
        let key_path = dir.join("identity.key");
        let pub_path = dir.join("identity.pub");

        if key_path.exists() {
            let raw = fs::read(&key_path).context("read identity.key")?;
            let bytes: [u8; 32] = raw
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid key length"))?;
            let secret = StaticSecret::from(bytes);
            let public = PublicKey::from(&secret);
            Ok(Self { secret, public })
        } else {
            let mut key_bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key_bytes);
            let secret = StaticSecret::from(key_bytes);
            let public = PublicKey::from(&secret);

            fs::write(&key_path, key_bytes)?;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
            fs::write(&pub_path, B64.encode(public.as_bytes()))?;

            tracing::info!("Generated new identity keypair");
            Ok(Self { secret, public })
        }
    }

    pub fn public_key_b64(&self) -> String {
        B64.encode(self.public.as_bytes())
    }

    fn acp_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".acp"))
    }
}

/// Derive a symmetric key from X25519 shared secret via HKDF-SHA256.
pub fn derive_key(my_secret: &StaticSecret, their_public: &PublicKey) -> [u8; 32] {
    let shared = my_secret.diffie_hellman(their_public);
    let hk = Hkdf::<Sha256>::new(Some(b"acp-v1"), shared.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(b"encryption", &mut key)
        .expect("HKDF expand failed");
    key
}

/// Encrypt a message with ChaCha20-Poly1305. Returns nonce || ciphertext (includes tag).
pub fn encrypt(key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad })
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt nonce || ciphertext. Returns plaintext.
pub fn decrypt(key: &[u8; 32], data: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 12 {
        anyhow::bail!("ciphertext too short");
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, Payload { msg: ciphertext, aad })
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))
}

/// Parse a base64-encoded X25519 public key.
pub fn parse_public_key(b64: &str) -> Result<PublicKey> {
    let bytes = B64.decode(b64).context("invalid base64 pubkey")?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("pubkey must be 32 bytes"))?;
    Ok(PublicKey::from(arr))
}
