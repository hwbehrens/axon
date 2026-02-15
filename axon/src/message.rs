use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_MESSAGE_SIZE: u32 = 65536;

pub type AgentId = String;

// ---------------------------------------------------------------------------
// MessageKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Hello,
    Ping,
    Pong,
    Query,
    Response,
    Delegate,
    Ack,
    Result,
    Notify,
    Cancel,
    Discover,
    Capabilities,
    Error,
}

impl MessageKind {
    pub fn expects_response(self) -> bool {
        matches!(
            self,
            MessageKind::Hello
                | MessageKind::Ping
                | MessageKind::Query
                | MessageKind::Delegate
                | MessageKind::Cancel
                | MessageKind::Discover
        )
    }

    pub fn is_response(self) -> bool {
        matches!(
            self,
            MessageKind::Pong
                | MessageKind::Response
                | MessageKind::Ack
                | MessageKind::Capabilities
                | MessageKind::Error
        )
    }

    pub fn is_required(self) -> bool {
        matches!(
            self,
            MessageKind::Hello
                | MessageKind::Ping
                | MessageKind::Pong
                | MessageKind::Query
                | MessageKind::Response
                | MessageKind::Notify
                | MessageKind::Error
        )
    }
}

impl fmt::Display for MessageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            MessageKind::Hello => "hello",
            MessageKind::Ping => "ping",
            MessageKind::Pong => "pong",
            MessageKind::Query => "query",
            MessageKind::Response => "response",
            MessageKind::Delegate => "delegate",
            MessageKind::Ack => "ack",
            MessageKind::Result => "result",
            MessageKind::Notify => "notify",
            MessageKind::Cancel => "cancel",
            MessageKind::Discover => "discover",
            MessageKind::Capabilities => "capabilities",
            MessageKind::Error => "error",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub id: Uuid,
    pub from: AgentId,
    pub to: AgentId,
    pub ts: u64,
    pub kind: MessageKind,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<Uuid>,
    pub payload: Value,
}

impl Envelope {
    pub fn new(from: AgentId, to: AgentId, kind: MessageKind, payload: Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            from,
            to,
            ts: now_millis(),
            kind,
            ref_id: None,
            payload,
        }
    }

    pub fn response_to(
        request: &Envelope,
        from: AgentId,
        kind: MessageKind,
        payload: Value,
    ) -> Self {
        Self {
            v: request.v,
            id: Uuid::new_v4(),
            from,
            to: request.from.clone(),
            ts: now_millis(),
            kind,
            ref_id: Some(request.id),
            payload,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.v == 0 {
            bail!("protocol version must be non-zero");
        }
        if self.from.len() != 32 || self.to.len() != 32 {
            bail!("agent IDs must be 32 hex chars");
        }
        if self.ts == 0 {
            bail!("timestamp must be non-zero");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wire format helpers
// ---------------------------------------------------------------------------

pub fn encode(envelope: &Envelope) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(envelope)?;
    let len = json.len() as u32;
    if len > MAX_MESSAGE_SIZE {
        bail!("message size {len} exceeds maximum {MAX_MESSAGE_SIZE}");
    }
    let mut buf = Vec::with_capacity(4 + json.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&json);
    Ok(buf)
}

pub fn decode(data: &[u8]) -> Result<Envelope> {
    let envelope: Envelope = serde_json::from_slice(data)?;
    Ok(envelope)
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX_EPOCH")
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Typed payload structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloPayload {
    pub protocol_versions: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingPayload {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PongPayload {
    pub status: PeerStatus,
    pub uptime_secs: u64,
    pub active_tasks: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryPayload {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsePayload {
    pub data: Value,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegatePayload {
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default = "default_true")]
    pub report_back: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AckPayload {
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultPayload {
    pub status: TaskStatus,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyPayload {
    pub topic: String,
    pub data: Value,
    #[serde(default)]
    pub importance: Importance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoverPayload {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilitiesPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_tasks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

// ---------------------------------------------------------------------------
// Typed enums for payload fields
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerStatus {
    Idle,
    Busy,
    Overloaded,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    Normal,
    Urgent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Completed,
    Failed,
    Partial,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotAuthorized,
    UnknownDomain,
    Overloaded,
    Internal,
    Timeout,
    Cancelled,
    IncompatibleVersion,
    UnknownKind,
    PeerNotFound,
}

pub fn hello_features() -> Vec<String> {
    vec![
        "delegate".to_string(),
        "ack".to_string(),
        "result".to_string(),
        "cancel".to_string(),
        "discover".to_string(),
        "capabilities".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent_a() -> String {
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string()
    }

    fn agent_b() -> String {
        "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
    }

    // --- Envelope basics ---

    #[test]
    fn envelope_round_trip() {
        let envelope = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Query,
            json!({"question": "hello", "domain": "meta.status"}),
        );
        let encoded = serde_json::to_string(&envelope).expect("serialize");
        let decoded: Envelope = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded.kind, MessageKind::Query);
        assert_eq!(decoded.payload["question"], json!("hello"));
    }

    #[test]
    fn response_links_request_id() {
        let req = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
        let resp = Envelope::response_to(
            &req,
            req.to.clone(),
            MessageKind::Pong,
            json!({"status":"idle", "uptime_secs": 0, "active_tasks": 0}),
        );
        assert_eq!(resp.ref_id, Some(req.id));
        assert_eq!(resp.to, req.from);
    }

    #[test]
    fn envelope_validation_catches_bad_ids() {
        let envelope = Envelope::new(
            "abc".to_string(),
            "def".to_string(),
            MessageKind::Notify,
            json!({"topic":"x", "data": {}}),
        );
        assert!(envelope.validate().is_err());
    }

    #[test]
    fn envelope_new_sets_defaults() {
        let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
        assert_eq!(env.v, 1);
        assert!(env.ref_id.is_none());
        assert!(env.ts > 0);
    }

    // --- Forward compatibility ---

    #[test]
    fn unknown_envelope_fields_are_ignored() {
        let raw = r#"{
            "v":1,
            "id":"6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "from":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "to":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "ts":1771108000000,
            "kind":"notify",
            "payload":{"topic":"meta.status","data":{}},
            "extra":"ignored"
        }"#;
        let decoded: Envelope = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(decoded.kind, MessageKind::Notify);
    }

    // --- MessageKind methods ---

    #[test]
    fn expects_response_mapping_matches_spec() {
        assert!(MessageKind::Hello.expects_response());
        assert!(MessageKind::Ping.expects_response());
        assert!(MessageKind::Query.expects_response());
        assert!(MessageKind::Delegate.expects_response());
        assert!(MessageKind::Cancel.expects_response());
        assert!(MessageKind::Discover.expects_response());
        assert!(!MessageKind::Notify.expects_response());
        assert!(!MessageKind::Result.expects_response());
        assert!(!MessageKind::Pong.expects_response());
        assert!(!MessageKind::Response.expects_response());
        assert!(!MessageKind::Ack.expects_response());
        assert!(!MessageKind::Capabilities.expects_response());
        assert!(!MessageKind::Error.expects_response());
    }

    #[test]
    fn is_response_mapping() {
        assert!(MessageKind::Pong.is_response());
        assert!(MessageKind::Response.is_response());
        assert!(MessageKind::Ack.is_response());
        assert!(MessageKind::Capabilities.is_response());
        assert!(MessageKind::Error.is_response());
        assert!(!MessageKind::Query.is_response());
        assert!(!MessageKind::Notify.is_response());
    }

    #[test]
    fn is_required_mapping() {
        assert!(MessageKind::Hello.is_required());
        assert!(MessageKind::Ping.is_required());
        assert!(MessageKind::Pong.is_required());
        assert!(MessageKind::Query.is_required());
        assert!(MessageKind::Response.is_required());
        assert!(MessageKind::Notify.is_required());
        assert!(MessageKind::Error.is_required());
        assert!(!MessageKind::Delegate.is_required());
        assert!(!MessageKind::Cancel.is_required());
        assert!(!MessageKind::Discover.is_required());
    }

    #[test]
    fn message_kind_display() {
        assert_eq!(MessageKind::Hello.to_string(), "hello");
        assert_eq!(MessageKind::Query.to_string(), "query");
        assert_eq!(MessageKind::Capabilities.to_string(), "capabilities");
    }

    // --- Wire format ---

    #[test]
    fn encode_decode_roundtrip() {
        let env = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Query,
            json!({"question": "test?"}),
        );
        let encoded = encode(&env).unwrap();
        let len = u32::from_be_bytes(encoded[..4].try_into().unwrap());
        assert_eq!(len as usize, encoded.len() - 4);
        let decoded = decode(&encoded[4..]).unwrap();
        assert_eq!(env, decoded);
    }

    #[test]
    fn reject_oversized_message() {
        let big = "x".repeat(MAX_MESSAGE_SIZE as usize);
        let env = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Query,
            json!({"question": big}),
        );
        let result = encode(&env);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    // --- Typed payload round-trips ---

    #[test]
    fn hello_payload_serde() {
        let p = HelloPayload {
            protocol_versions: vec![1],
            selected_version: None,
            agent_name: Some("Test".to_string()),
            features: vec!["delegate".to_string()],
        };
        let v = serde_json::to_value(&p).unwrap();
        let back: HelloPayload = serde_json::from_value(v).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn hello_response_payload_serde() {
        let p = HelloPayload {
            protocol_versions: vec![1],
            selected_version: Some(1),
            agent_name: None,
            features: vec![],
        };
        let v = serde_json::to_value(&p).unwrap();
        let back: HelloPayload = serde_json::from_value(v).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn ping_pong_payload_serde() {
        let ping = PingPayload {};
        let v = serde_json::to_value(&ping).unwrap();
        let _: PingPayload = serde_json::from_value(v).unwrap();

        let pong = PongPayload {
            status: PeerStatus::Idle,
            uptime_secs: 3600,
            active_tasks: 2,
            agent_name: Some("Bot".to_string()),
        };
        let v = serde_json::to_value(&pong).unwrap();
        let back: PongPayload = serde_json::from_value(v).unwrap();
        assert_eq!(pong, back);
    }

    #[test]
    fn query_response_payload_serde() {
        let q = QueryPayload {
            question: "test?".to_string(),
            domain: Some("meta.status".to_string()),
            max_tokens: Some(200),
            deadline_ms: Some(30000),
        };
        let v = serde_json::to_value(&q).unwrap();
        let back: QueryPayload = serde_json::from_value(v).unwrap();
        assert_eq!(q, back);

        let r = ResponsePayload {
            data: json!({"events": []}),
            summary: "None".to_string(),
            tokens_used: Some(5),
            truncated: Some(false),
        };
        let v = serde_json::to_value(&r).unwrap();
        let back: ResponsePayload = serde_json::from_value(v).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn delegate_payload_defaults() {
        let json = json!({"task": "do something"});
        let d: DelegatePayload = serde_json::from_value(json).unwrap();
        assert_eq!(d.priority, Priority::Normal);
        assert!(d.report_back);
        assert!(d.context.is_none());
        assert!(d.deadline_ms.is_none());
    }

    #[test]
    fn ack_result_payload_serde() {
        let ack = AckPayload {
            accepted: true,
            estimated_ms: Some(5000),
        };
        let v = serde_json::to_value(&ack).unwrap();
        let back: AckPayload = serde_json::from_value(v).unwrap();
        assert_eq!(ack, back);

        let res = ResultPayload {
            status: TaskStatus::Completed,
            outcome: "Done".to_string(),
            data: None,
            error: None,
        };
        let v = serde_json::to_value(&res).unwrap();
        let back: ResultPayload = serde_json::from_value(v).unwrap();
        assert_eq!(res, back);
    }

    #[test]
    fn result_failed_payload_serde() {
        let res = ResultPayload {
            status: TaskStatus::Failed,
            outcome: "Could not send".to_string(),
            data: None,
            error: Some("Service unavailable".to_string()),
        };
        let v = serde_json::to_value(&res).unwrap();
        let back: ResultPayload = serde_json::from_value(v).unwrap();
        assert_eq!(res, back);
    }

    #[test]
    fn notify_payload_defaults() {
        let json = json!({"topic": "test", "data": {}});
        let n: NotifyPayload = serde_json::from_value(json).unwrap();
        assert_eq!(n.importance, Importance::Low);
    }

    #[test]
    fn cancel_payload_serde() {
        let c = CancelPayload {
            reason: Some("Plans changed".to_string()),
        };
        let v = serde_json::to_value(&c).unwrap();
        let back: CancelPayload = serde_json::from_value(v).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn discover_capabilities_payload_serde() {
        let d = DiscoverPayload {};
        let v = serde_json::to_value(&d).unwrap();
        let _: DiscoverPayload = serde_json::from_value(v).unwrap();

        let caps = CapabilitiesPayload {
            agent_name: Some("Family Assistant".to_string()),
            domains: vec!["family".to_string()],
            channels: vec!["imessage".to_string()],
            tools: vec!["web_search".to_string()],
            max_concurrent_tasks: Some(4),
            model: Some("gemini-3-pro".to_string()),
        };
        let v = serde_json::to_value(&caps).unwrap();
        let back: CapabilitiesPayload = serde_json::from_value(v).unwrap();
        assert_eq!(caps, back);
    }

    #[test]
    fn error_payload_serde() {
        let e = ErrorPayload {
            code: ErrorCode::UnknownDomain,
            message: "I don't handle that".to_string(),
            retryable: false,
        };
        let v = serde_json::to_value(&e).unwrap();
        let back: ErrorPayload = serde_json::from_value(v).unwrap();
        assert_eq!(e, back);
    }

    // --- Enum serialization ---

    #[test]
    fn peer_status_snake_case() {
        assert_eq!(serde_json::to_string(&PeerStatus::Idle).unwrap(), "\"idle\"");
        assert_eq!(serde_json::to_string(&PeerStatus::Busy).unwrap(), "\"busy\"");
        assert_eq!(serde_json::to_string(&PeerStatus::Overloaded).unwrap(), "\"overloaded\"");
    }

    #[test]
    fn priority_snake_case() {
        assert_eq!(serde_json::to_string(&Priority::Normal).unwrap(), "\"normal\"");
        assert_eq!(serde_json::to_string(&Priority::Urgent).unwrap(), "\"urgent\"");
    }

    #[test]
    fn task_status_snake_case() {
        assert_eq!(serde_json::to_string(&TaskStatus::Completed).unwrap(), "\"completed\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Failed).unwrap(), "\"failed\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Partial).unwrap(), "\"partial\"");
    }

    #[test]
    fn importance_snake_case() {
        assert_eq!(serde_json::to_string(&Importance::Low).unwrap(), "\"low\"");
        assert_eq!(serde_json::to_string(&Importance::Medium).unwrap(), "\"medium\"");
        assert_eq!(serde_json::to_string(&Importance::High).unwrap(), "\"high\"");
    }

    #[test]
    fn error_code_snake_case() {
        assert_eq!(serde_json::to_string(&ErrorCode::NotAuthorized).unwrap(), "\"not_authorized\"");
        assert_eq!(serde_json::to_string(&ErrorCode::IncompatibleVersion).unwrap(), "\"incompatible_version\"");
        assert_eq!(serde_json::to_string(&ErrorCode::PeerNotFound).unwrap(), "\"peer_not_found\"");
    }

    // --- ref field serializes correctly ---

    #[test]
    fn ref_field_serializes_as_ref_not_ref_id() {
        let env = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
        let v = serde_json::to_value(&env).unwrap();
        assert!(v.get("ref").is_some() || v.get("ref_id").is_none());
        // ref should be null when not set
        assert!(v["ref"].is_null());
    }

    #[test]
    fn ref_field_present_when_set() {
        let req = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
        let resp = Envelope::response_to(
            &req,
            agent_b(),
            MessageKind::Pong,
            json!({"status":"idle","uptime_secs":0,"active_tasks":0}),
        );
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["ref"].as_str().unwrap(), req.id.to_string());
    }

    // --- hello_features ---

    #[test]
    fn hello_features_includes_all_optional_kinds() {
        let f = hello_features();
        assert!(f.contains(&"delegate".to_string()));
        assert!(f.contains(&"cancel".to_string()));
        assert!(f.contains(&"discover".to_string()));
        assert!(f.contains(&"capabilities".to_string()));
        assert!(f.contains(&"ack".to_string()));
        assert!(f.contains(&"result".to_string()));
    }

    // --- now_millis ---

    #[test]
    fn now_millis_is_plausible() {
        let ms = now_millis();
        // After 2020-01-01
        assert!(ms > 1_577_836_800_000);
    }
}
