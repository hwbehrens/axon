use anyhow::{Context, Result};
use base64::Engine;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rcgen::{Certificate, CertificateParams, KeyPair, SanType};
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::fs;

pub const IDENTITY_DIR: &str = ".axon";
pub const KEY_FILE: &str = "identity.key";
pub const PUB_FILE: &str = "identity.pub";

pub struct Identity {
    pub signing_key: SigningKey,
}

impl Identity {
    pub fn generate() -> Self {
        let mut csprng = rand::thread_rng();
        let signing_key = SigningKey::generate(&mut csprng);
        Self { signing_key }
    }

    pub async fn load_or_generate(base_dir: &Path) -> Result<Self> {
        let axon_dir = base_dir.join(IDENTITY_DIR);
        if !axon_dir.exists() {
            fs::create_dir_all(&axon_dir).await?;
        }

        let key_path = axon_dir.join(KEY_FILE);
        if key_path.exists() {
            let bytes = fs::read(&key_path).await?;
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow::anyhow!("Invalid key length"))?;
            let signing_key = SigningKey::from_bytes(&bytes);
            Ok(Self { signing_key })
        } else {
            let identity = Self::generate();
            fs::write(&key_path, identity.signing_key.to_bytes()).await?;
            
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).await?;
            }

            let pub_path = axon_dir.join(PUB_FILE);
            let pub_key_base64 = base64::engine::general_purpose::STANDARD.encode(identity.verifying_key().to_bytes());
            fs::write(&pub_path, pub_key_base64).await?;

            Ok(identity)
        }
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn agent_id(&self) -> String {
        let pub_key_bytes = self.verifying_key().to_bytes();
        let mut hasher = Sha256::new();
        hasher.update(pub_key_bytes);
        let result = hasher.finalize();
        hex::encode(&result[..16])
    }

    pub fn self_signed_cert(&self) -> Result<Certificate> {
        let mut params = CertificateParams::default();
        params.alg = &rcgen::PKCS_ED25519;
        
        let agent_id = self.agent_id();
        params.subject_alt_names = vec![
            SanType::DnsName(agent_id),
            SanType::DnsName("localhost".to_string()),
        ];
        
        let key_pair = KeyPair::from_der(&self.signing_key.to_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to create KeyPair: {}", e))?;
        params.key_pair = Some(key_pair);

        Certificate::from_params(params).map_err(|e| anyhow::anyhow!("Failed to generate cert: {}", e))
    }
}
