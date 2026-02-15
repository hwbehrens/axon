use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u32,
    pub id: Uuid,
    pub from: String,
    pub to: String,
    pub ts: u64,
    pub kind: String,
    #[serde(rename = "ref")]
    pub ref_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

impl Envelope {
    pub fn new(from: String, to: String, kind: String, payload: serde_json::Value) -> Self {
        Self {
            v: 1,
            id: Uuid::new_v4(),
            from,
            to,
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            kind,
            ref_id: None,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloPayload {
    pub protocol_versions: Vec<u32>,
    pub agent_name: Option<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResponsePayload {
    pub protocol_versions: Vec<u32>,
    pub selected_version: u32,
    pub agent_name: Option<String>,
    pub features: Vec<String>,
}
