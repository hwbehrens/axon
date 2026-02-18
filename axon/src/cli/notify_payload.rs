use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub fn parse_notify_payload(data: &str, parse_json: bool) -> Result<Value> {
    if !parse_json {
        return Ok(json!(data));
    }

    serde_json::from_str(data).map_err(|err| anyhow!("invalid JSON for --json payload: {err}"))
}

#[cfg(test)]
#[path = "notify_payload_tests.rs"]
mod tests;
