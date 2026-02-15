use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};

use super::envelope::Envelope;

pub const MAX_MESSAGE_SIZE: u32 = 65536;

pub fn encode(envelope: &Envelope) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(envelope)?;
    if json.len() > MAX_MESSAGE_SIZE as usize {
        bail!(
            "message size {} exceeds maximum {MAX_MESSAGE_SIZE}",
            json.len()
        );
    }
    Ok(json)
}

pub fn decode(data: &[u8]) -> Result<Envelope> {
    if data.len() > MAX_MESSAGE_SIZE as usize {
        bail!(
            "message size {} exceeds maximum {MAX_MESSAGE_SIZE}",
            data.len()
        );
    }
    let envelope: Envelope = serde_json::from_slice(data)?;
    Ok(envelope)
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
