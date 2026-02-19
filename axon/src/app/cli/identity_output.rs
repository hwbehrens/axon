use anyhow::{Context, Result};
use serde_json::json;

pub fn render_identity_human(uri: &str) -> String {
    format!("Your enrollment token (share with peers):\n{uri}")
}

pub fn render_identity_json(
    agent_id: &str,
    public_key: &str,
    addr: &str,
    port: u16,
    uri: &str,
) -> Result<String> {
    serde_json::to_string_pretty(&json!({
        "agent_id": agent_id,
        "public_key": public_key,
        "addr": addr,
        "port": port,
        "uri": uri,
    }))
    .context("failed to encode identity output")
}
