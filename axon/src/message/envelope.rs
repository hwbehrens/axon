use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use uuid::Uuid;

use super::kind::MessageKind;
use super::wire::now_millis;

pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for AgentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for AgentId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl AsRef<str> for AgentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for AgentId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for AgentId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for AgentId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for AgentId {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

impl serde::Serialize for AgentId {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for AgentId {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub id: Uuid,
    pub from: AgentId,
    pub to: AgentId,
    pub ts: u64,
    pub kind: MessageKind,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<Uuid>,
    pub payload: Box<RawValue>,
}

impl PartialEq for Envelope {
    fn eq(&self, other: &Self) -> bool {
        self.v == other.v
            && self.id == other.id
            && self.from == other.from
            && self.to == other.to
            && self.ts == other.ts
            && self.kind == other.kind
            && self.ref_id == other.ref_id
            && self.payload.get() == other.payload.get()
    }
}

impl Envelope {
    /// Create a payload from a serde_json::Value by serializing it to raw JSON.
    pub fn raw_json(value: &Value) -> Box<RawValue> {
        RawValue::from_string(serde_json::to_string(value).expect("Value serializes to JSON"))
            .expect("valid JSON")
    }

    /// Parse the payload into a serde_json::Value (for inspection).
    pub fn payload_value(&self) -> Value {
        serde_json::from_str(self.payload.get()).unwrap_or(Value::Null)
    }

    /// Parse the payload into a typed struct.
    pub fn payload_as<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_str(self.payload.get())?)
    }

    pub fn new(
        from: impl Into<AgentId>,
        to: impl Into<AgentId>,
        kind: MessageKind,
        payload: Value,
    ) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            from: from.into(),
            to: to.into(),
            ts: now_millis(),
            kind,
            ref_id: None,
            payload: Self::raw_json(&payload),
        }
    }

    pub fn response_to(
        request: &Envelope,
        from: impl Into<AgentId>,
        kind: MessageKind,
        payload: Value,
    ) -> Self {
        Self {
            v: request.v,
            id: Uuid::new_v4(),
            from: from.into(),
            to: request.from.clone(),
            ts: now_millis(),
            kind,
            ref_id: Some(request.id),
            payload: Self::raw_json(&payload),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.v == 0 {
            bail!("protocol version must be non-zero");
        }
        if !Self::is_valid_agent_id(&self.from) || !Self::is_valid_agent_id(&self.to) {
            bail!("agent IDs must be in the format ed25519.<32 hex chars>");
        }
        if self.ts == 0 {
            bail!("timestamp must be non-zero");
        }
        Ok(())
    }

    fn is_valid_agent_id(id: &str) -> bool {
        let Some(hex) = id.strip_prefix("ed25519.") else {
            return false;
        };
        hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit())
    }
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
