use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use uuid::Uuid;

use super::kind::MessageKind;

/// Typed agent identity string (e.g. `ed25519.<32 hex chars>`).
///
/// See `spec/SPEC.md` §1 and `spec/WIRE_FORMAT.md` §3 for derivation rules.
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

impl std::borrow::Borrow<str> for AgentId {
    fn borrow(&self) -> &str {
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

/// AXON wire envelope — the top-level JSON object for every QUIC message.
///
/// The wire format carries only `id`, `kind`, `payload`, and optionally `ref`.
/// The `from` and `to` fields are populated by the daemon layer (not on wire)
/// for IPC client consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: Uuid,
    pub kind: MessageKind,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<Uuid>,
    pub payload: Box<RawValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<AgentId>,
}

impl PartialEq for Envelope {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.kind == other.kind
            && self.ref_id == other.ref_id
            && self.payload.get() == other.payload.get()
            && self.from == other.from
            && self.to == other.to
    }
}

impl Envelope {
    /// Create a payload from a serde_json::Value by serializing it to raw JSON.
    pub fn raw_json(value: &Value) -> Box<RawValue> {
        RawValue::from_string(serde_json::to_string(value).expect("Value serializes to JSON"))
            .expect("valid JSON")
    }

    /// Parse the payload into a serde_json::Value (for inspection).
    pub fn payload_value(&self) -> Result<Value> {
        Ok(serde_json::from_str(self.payload.get())?)
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
            id: Uuid::new_v4(),
            kind,
            ref_id: None,
            payload: Self::raw_json(&payload),
            from: Some(from.into()),
            to: Some(to.into()),
        }
    }

    pub fn response_to(
        request: &Envelope,
        from: impl Into<AgentId>,
        kind: MessageKind,
        payload: Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            ref_id: Some(request.id),
            payload: Self::raw_json(&payload),
            from: Some(from.into()),
            to: request.from.clone(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.is_nil() {
            bail!("message id must be non-nil");
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
