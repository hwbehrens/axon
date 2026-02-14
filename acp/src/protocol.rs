use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub id: String,
    pub from: String,
    pub to: String,
    pub ts: u64,
    pub kind: MessageKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    Query,
    Response,
    Delegate,
    Notify,
}

impl Envelope {
    pub fn new_query(from: &str, to: &str, message: &str) -> Self {
        Self {
            v: 1,
            id: Uuid::new_v4().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            ts: now_millis(),
            kind: MessageKind::Query,
            reply_to: None,
            payload: serde_json::json!({
                "question": message,
                "context_budget": "standard"
            }),
        }
    }

    pub fn new_delegate(from: &str, to: &str, task: &str) -> Self {
        Self {
            v: 1,
            id: Uuid::new_v4().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            ts: now_millis(),
            kind: MessageKind::Delegate,
            reply_to: None,
            payload: serde_json::json!({
                "task": task,
                "priority": "normal",
                "report_back": true
            }),
        }
    }

    pub fn new_notify(from: &str, topic: &str, data: &str) -> Self {
        Self {
            v: 1,
            id: Uuid::new_v4().to_string(),
            from: from.to_string(),
            to: "".to_string(),
            ts: now_millis(),
            kind: MessageKind::Notify,
            reply_to: None,
            payload: serde_json::json!({
                "topic": topic,
                "data": data
            }),
        }
    }

    /// AAD for AEAD: from, to, ts fields.
    pub fn aad(&self) -> Vec<u8> {
        format!("{}:{}:{}", self.from, self.to, self.ts).into_bytes()
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
