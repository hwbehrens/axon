use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// AXON message kind — determines stream mapping.
///
/// - `Request` → bidirectional stream (expects a `Response` or `Error`)
/// - `Response` → bidirectional stream (reply to a `Request`)
/// - `Message` → unidirectional stream (fire-and-forget)
/// - `Error` → bidirectional stream (error reply to a `Request`)
/// - `Describe` → bidirectional stream (capability-manifest query; answered
///   by the receiving daemon from its registered manifest, never delivered
///   to the application handler)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MessageKind {
    Request,
    Response,
    Message,
    Error,
    Describe,
    Unknown(Box<str>),
}

impl MessageKind {
    pub fn expects_response(&self) -> bool {
        matches!(self, MessageKind::Request | MessageKind::Describe)
    }

    pub fn is_response(&self) -> bool {
        matches!(self, MessageKind::Response | MessageKind::Error)
    }

    pub fn is_allowed_on_unidirectional(&self) -> bool {
        matches!(
            self,
            MessageKind::Message | MessageKind::Error | MessageKind::Unknown(_)
        )
    }

    pub fn unknown(value: impl Into<Box<str>>) -> Self {
        Self::Unknown(value.into())
    }

    pub fn as_str(&self) -> &str {
        match self {
            MessageKind::Request => "request",
            MessageKind::Response => "response",
            MessageKind::Message => "message",
            MessageKind::Error => "error",
            MessageKind::Describe => "describe",
            MessageKind::Unknown(value) => value,
        }
    }
}

impl fmt::Display for MessageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for MessageKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MessageKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "request" => Self::Request,
            "response" => Self::Response,
            "message" => Self::Message,
            "error" => Self::Error,
            "describe" => Self::Describe,
            _ => Self::Unknown(value.into_boxed_str()),
        })
    }
}

/// Typed agent identity string (e.g. `ed25519.<32 hex chars>`).
///
/// See `spec/SPEC.md` §1 and `spec/WIRE_FORMAT.md` §2.2 for derivation rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentId(String);

impl AgentId {
    pub const PREFIX: &'static str = "ed25519.";
    pub const HEX_LENGTH: usize = 32;

    pub fn parse(input: &str) -> Result<Self> {
        let (prefix, hex) = input
            .split_once('.')
            .ok_or_else(|| anyhow::anyhow!("agent_id must contain an algorithm prefix"))?;
        if !prefix.eq_ignore_ascii_case("ed25519") {
            bail!("unsupported agent_id algorithm '{prefix}'");
        }
        if hex.len() != Self::HEX_LENGTH || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("agent_id must contain exactly 32 hexadecimal characters");
        }
        Ok(Self(format!(
            "{}{}",
            Self::PREFIX,
            hex.to_ascii_lowercase()
        )))
    }

    pub fn from_pubkey_bytes(pubkey: &[u8]) -> Result<Self> {
        if pubkey.len() != 32 {
            bail!(
                "Ed25519 public key must contain exactly 32 bytes, got {}",
                pubkey.len()
            );
        }
        let digest = Sha256::digest(pubkey);
        let hex: String = digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self(format!("{}{hex}", Self::PREFIX)))
    }

    pub fn from_pubkey_base64(pubkey: &str) -> Result<Self> {
        let bytes = STANDARD
            .decode(pubkey.trim())
            .map_err(|err| anyhow::anyhow!("public key is not valid base64: {err}"))?;
        Self::from_pubkey_bytes(&bytes)
    }

    pub fn matches_pubkey_base64(&self, pubkey: &str) -> Result<bool> {
        Ok(*self == Self::from_pubkey_base64(pubkey)?)
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

impl TryFrom<String> for AgentId {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl TryFrom<&str> for AgentId {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        Self::parse(value)
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
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
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

    pub fn new(from: AgentId, to: AgentId, kind: MessageKind, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            ref_id: None,
            payload: Self::raw_json(&payload),
            from: Some(from),
            to: Some(to),
        }
    }

    pub fn response_to(
        request: &Envelope,
        from: AgentId,
        kind: MessageKind,
        payload: Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            ref_id: Some(request.id),
            payload: Self::raw_json(&payload),
            from: Some(from),
            to: request.from.clone(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.is_nil() {
            bail!("message id must be non-nil");
        }
        if self.id.get_version() != Some(uuid::Version::Random) {
            bail!("message id must be UUID v4");
        }
        if !self.payload.get().trim_start().starts_with('{') {
            bail!("payload must be a JSON object");
        }
        Ok(())
    }

    /// Serialize for QUIC wire transport.
    ///
    /// The wire format carries only `id`, `kind`, `payload`, and optional
    /// `ref`; daemon-local routing fields (`from`, `to`) are stripped.
    pub fn wire_encode(&self) -> Result<Vec<u8>> {
        let mut wire = self.clone();
        wire.from = None;
        wire.to = None;
        encode(&wire)
    }
}

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
#[path = "envelope_tests.rs"]
mod tests;
