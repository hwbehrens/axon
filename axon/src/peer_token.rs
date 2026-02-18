use anyhow::{Context, Result, anyhow};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use sha2::{Digest, Sha256};

use crate::message::AgentId;

const SCHEME_PREFIX: &str = "axon://";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedPeerToken {
    pub pubkey: String,
    pub addr: String,
    pub agent_id: AgentId,
}

pub fn encode(pubkey_base64: &str, addr: &str) -> Result<String> {
    let key_bytes = STANDARD
        .decode(pubkey_base64.trim())
        .context("public_key is not valid base64")?;
    ensure_pubkey_length(&key_bytes)?;
    validate_addr(addr)?;
    let key_url = URL_SAFE_NO_PAD.encode(key_bytes);
    Ok(format!("{SCHEME_PREFIX}{key_url}@{}", addr.trim()))
}

pub fn decode(token: &str) -> Result<DecodedPeerToken> {
    let rest = token
        .strip_prefix(SCHEME_PREFIX)
        .ok_or_else(|| anyhow!("peer token must start with '{SCHEME_PREFIX}'"))?;
    let (pubkey_url, addr_raw) = rest
        .split_once('@')
        .ok_or_else(|| anyhow!("peer token must contain '@' between pubkey and addr"))?;
    if pubkey_url.is_empty() {
        anyhow::bail!("peer token pubkey segment is empty");
    }
    validate_addr(addr_raw)?;

    let key_bytes = URL_SAFE_NO_PAD
        .decode(pubkey_url)
        .context("peer token pubkey is not valid base64url")?;
    let (pubkey, agent_id) = decode_pubkey_bytes(&key_bytes)?;

    Ok(DecodedPeerToken {
        pubkey,
        addr: addr_raw.trim().to_string(),
        agent_id,
    })
}

pub fn derive_agent_id_from_pubkey_base64(pubkey_base64: &str) -> Result<AgentId> {
    let key_bytes = STANDARD
        .decode(pubkey_base64.trim())
        .context("pubkey is not valid base64")?;
    let (_, agent_id) = decode_pubkey_bytes(&key_bytes)?;
    Ok(agent_id)
}

fn ensure_pubkey_length(key_bytes: &[u8]) -> Result<()> {
    if key_bytes.len() != 32 {
        anyhow::bail!(
            "peer token pubkey must decode to 32 bytes, got {}",
            key_bytes.len()
        );
    }
    Ok(())
}

fn decode_pubkey_bytes(key_bytes: &[u8]) -> Result<(String, AgentId)> {
    ensure_pubkey_length(key_bytes)?;
    let pubkey = STANDARD.encode(key_bytes);
    let agent_id = derive_agent_id(key_bytes);
    Ok((pubkey, agent_id))
}

fn validate_addr(addr: &str) -> Result<()> {
    let addr = addr.trim();
    if addr.is_empty() {
        anyhow::bail!("peer token addr cannot be empty");
    }
    if addr.parse::<std::net::SocketAddr>().is_ok() {
        return Ok(());
    }
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("peer token addr must be host:port or ip:port"))?;
    if host.is_empty() {
        anyhow::bail!("peer token addr host cannot be empty");
    }
    let _ = port
        .parse::<u16>()
        .with_context(|| format!("peer token addr has invalid port '{port}'"))?;
    Ok(())
}

fn derive_agent_id(key_bytes: &[u8]) -> AgentId {
    let digest = Sha256::digest(key_bytes);
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    AgentId::from(format!("ed25519.{hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encodes_and_decodes() {
        let pubkey = STANDARD.encode([7u8; 32]);
        let token = encode(&pubkey, "127.0.0.1:7100").expect("encode");
        assert!(token.starts_with("axon://"));

        let decoded = decode(&token).expect("decode");
        assert_eq!(decoded.pubkey, pubkey);
        assert_eq!(decoded.addr, "127.0.0.1:7100");
        assert!(decoded.agent_id.as_str().starts_with("ed25519."));
    }

    #[test]
    fn decode_rejects_bad_scheme() {
        let err = decode("http://abc@127.0.0.1:7100").expect_err("bad scheme should fail");
        assert!(err.to_string().contains("must start"));
    }

    #[test]
    fn decode_rejects_invalid_pubkey_b64url() {
        let err = decode("axon://@@127.0.0.1:7100").expect_err("bad pubkey should fail");
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn decode_rejects_invalid_pubkey_length() {
        let short = URL_SAFE_NO_PAD.encode([1u8; 8]);
        let err = decode(&format!("axon://{short}@127.0.0.1:7100"))
            .expect_err("short pubkey should fail");
        assert!(err.to_string().contains("32 bytes"));
    }

    #[test]
    fn encode_rejects_malformed_addr() {
        let pubkey = STANDARD.encode([7u8; 32]);
        let err = encode(&pubkey, "host-without-port").expect_err("bad addr should fail");
        assert!(err.to_string().contains("host:port"));
    }

    #[test]
    fn derive_agent_id_from_base64_rejects_wrong_length() {
        let pubkey = STANDARD.encode([1u8; 8]);
        let err = derive_agent_id_from_pubkey_base64(&pubkey).expect_err("short key");
        assert!(err.to_string().contains("32 bytes"));
    }
}
