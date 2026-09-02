use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::manifest::Manifest;
use crate::message::{AgentId, Envelope, MessageKind};

pub const MAX_IPC_LINE_LENGTH: usize = 64 * 1024;

/// Upper bound on the echoed `req_id` string (spec/IPC.md §3). Commands
/// carrying a longer `req_id` are rejected with `invalid_command` before
/// dispatch: an unbounded echo would let a legal (under the line limit)
/// command produce error and reply frames that exceed the limit — including
/// the `message_too_large` fallback itself, which must never be oversized.
pub const MAX_REQ_ID_BYTES: usize = 1024;

/// Failure to encode an outbound daemon reply/event as one spec-conformant
/// IPC line.
#[derive(Debug)]
pub enum EncodeLineError {
    /// Serialization failed unexpectedly (all reply types are plain JSON).
    Serialize(String),
    /// The encoded line plus its trailing newline would exceed
    /// [`MAX_IPC_LINE_LENGTH`].
    TooLarge(usize),
}

/// Serialize one outbound reply or event into a framed IPC line.
///
/// The 65,536-byte limit INCLUDES the trailing newline (spec/IPC.md §2), so
/// the JSON body may be at most 65,535 bytes. Oversized payloads are refused
/// — never truncated — so callers can fail delivery explicitly (an error
/// reply, or a logged drop). Without this check a network envelope accepted
/// under the wire limit becomes an oversized IPC line once wrapped in event
/// JSON, and the client's reader would reject or choke on the frame.
pub fn encode_reply_line(reply: &DaemonReply) -> Result<Arc<str>, EncodeLineError> {
    let serialized =
        serde_json::to_string(reply).map_err(|err| EncodeLineError::Serialize(err.to_string()))?;
    if serialized.len() + 1 > MAX_IPC_LINE_LENGTH {
        return Err(EncodeLineError::TooLarge(serialized.len() + 1));
    }
    Ok(Arc::from(serialized))
}

/// Encode a terminal error reply, guaranteed to fit the framed limit.
///
/// If the echoed `req_id` would push the line past the limit (only possible
/// for callers that bypass the ingress bound, e.g. in-process tests), the
/// correlation echo is DROPPED and the minimal error line is encoded —
/// never truncated, never a panic. Error bodies are static text, so the
/// no-echo form always fits.
pub fn error_reply_line(
    code: IpcErrorCode,
    req_id: Option<String>,
) -> Result<Arc<str>, EncodeLineError> {
    let reply = |req_id: Option<String>| DaemonReply::Error {
        ok: false,
        error: code,
        message: code.message().to_string(),
        req_id,
    };
    match encode_reply_line(&reply(req_id)) {
        Ok(line) => Ok(line),
        Err(EncodeLineError::TooLarge(_)) => encode_reply_line(&reply(None)),
        Err(err) => Err(err),
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpcSendKind {
    Request,
    Message,
    /// Capability-manifest query. Answered by the *receiving daemon* from
    /// its registered manifest; the remote application handler is never
    /// woken (spec/MESSAGE_TYPES.md §describe).
    Describe,
}

impl IpcSendKind {
    pub fn as_message_kind(self) -> MessageKind {
        match self {
            Self::Request => MessageKind::Request,
            Self::Message => MessageKind::Message,
            Self::Describe => MessageKind::Describe,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpcReplyKind {
    Response,
    Error,
}

impl IpcReplyKind {
    pub fn as_message_kind(self) -> MessageKind {
        match self {
            Self::Response => MessageKind::Response,
            Self::Error => MessageKind::Error,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcCommand {
    Send {
        to: AgentId,
        kind: IpcSendKind,
        payload: Value,
        #[serde(default)]
        timeout_secs: Option<u64>,
        #[serde(default, rename = "ref")]
        ref_id: Option<Uuid>,
        #[serde(default)]
        req_id: Option<String>,
    },
    Peers {
        #[serde(default)]
        req_id: Option<String>,
    },
    Status {
        #[serde(default)]
        req_id: Option<String>,
    },
    Whoami {
        #[serde(default)]
        req_id: Option<String>,
    },
    AddPeer {
        #[serde(default)]
        agent_id: Option<AgentId>,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        req_id: Option<String>,
    },
    RemovePeer {
        agent_id: AgentId,
        #[serde(default)]
        req_id: Option<String>,
    },
    WhoCan {
        /// Case-insensitive substring matched against service ids and
        /// descriptions. Absent/empty lists every reachable service.
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        req_id: Option<String>,
    },
    Serve {
        /// Optional capability manifest published with the handler lease.
        /// The daemon answers inbound `describe` requests from it without
        /// waking this client. Validated at parse time and size-bounded at
        /// dispatch time (spec/IPC.md §4.7).
        #[serde(default)]
        manifest: Option<Manifest>,
        #[serde(default)]
        req_id: Option<String>,
    },
    Reply {
        request_id: Uuid,
        /// Authenticated origin of the request being answered (the `from`
        /// delivered with the request event). Optional: when omitted and the
        /// request ID matches several peer-scoped pending requests, the reply
        /// is rejected as ambiguous instead of hitting an arbitrary peer.
        #[serde(default)]
        peer: Option<AgentId>,
        kind: IpcReplyKind,
        payload: Value,
        #[serde(default)]
        req_id: Option<String>,
    },
}

impl IpcCommand {
    pub fn req_id(&self) -> Option<&str> {
        match self {
            Self::Send { req_id, .. }
            | Self::Peers { req_id }
            | Self::Status { req_id }
            | Self::Whoami { req_id }
            | Self::AddPeer { req_id, .. }
            | Self::RemovePeer { req_id, .. }
            | Self::WhoCan { req_id, .. }
            | Self::Serve { req_id, .. }
            | Self::Reply { req_id, .. } => req_id.as_deref(),
        }
    }

    pub fn cmd_name(&self) -> &'static str {
        match self {
            Self::Send { .. } => "send",
            Self::Peers { .. } => "peers",
            Self::Status { .. } => "status",
            Self::Whoami { .. } => "whoami",
            Self::AddPeer { .. } => "add_peer",
            Self::RemovePeer { .. } => "remove_peer",
            Self::WhoCan { .. } => "who_can",
            Self::Serve { .. } => "serve",
            Self::Reply { .. } => "reply",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandEvent {
    pub client_id: u64,
    pub command: IpcCommand,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerSummary {
    pub agent_id: String,
    pub public_key: String,
    pub trust: &'static str,
    pub locators: Vec<String>,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Advisory service ids from the peer's last observed capability
    /// manifest. Omitted when no manifest has been observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhoamiInfo {
    pub agent_id: String,
    pub public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceSummary {
    pub id: String,
    pub description: String,
}

/// One peer's services matching a `who_can` query.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceMatch {
    pub agent_id: String,
    pub services: Vec<ServiceSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    InvalidCommand,
    CommandTooLarge,
    PeerNotFound,
    PeerNotObserved,
    PeerConflict,
    SelfSend,
    PeerUnreachable,
    Timeout,
    HandlerBusy,
    NotHandler,
    RequestNotFound,
    SendCapacityExceeded,
    MessageTooLarge,
    InternalError,
}

impl IpcErrorCode {
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidCommand => "malformed command or invalid field combination",
            Self::CommandTooLarge => "IPC command exceeds 64KB",
            Self::PeerNotFound => "target is not an enrolled peer",
            Self::PeerNotObserved => "candidate is not currently observed",
            Self::PeerConflict => "Agent ID conflicts with an enrolled public key",
            Self::SelfSend => "cannot enroll or send to the local Agent ID",
            Self::PeerUnreachable => "peer is enrolled but unreachable",
            Self::Timeout => "request timed out waiting for a response",
            Self::HandlerBusy => "another IPC connection owns the request-handler lease",
            Self::NotHandler => "this IPC connection does not own the request-handler lease",
            Self::RequestNotFound => "request is unknown, expired, or already completed",
            Self::SendCapacityExceeded => "too many concurrent sends; retry shortly",
            Self::MessageTooLarge => "reply or event exceeds the 64KB IPC line limit",
            Self::InternalError => "unexpected daemon or persistence failure",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DaemonReply {
    SendOk {
        ok: bool,
        msg_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        response: Option<Envelope>,
    },
    Peers {
        ok: bool,
        peers: Vec<PeerSummary>,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Status {
        ok: bool,
        uptime_secs: u64,
        peers_connected: usize,
        messages_sent: u64,
        messages_received: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Whoami {
        ok: bool,
        #[serde(flatten)]
        info: WhoamiInfo,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    PeerChanged {
        ok: bool,
        agent_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Serving {
        ok: bool,
        serving: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Replied {
        ok: bool,
        request_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    WhoCan {
        ok: bool,
        matches: Vec<ServiceMatch>,
        /// Connected enrolled peers that failed to answer a capability pull;
        /// named explicitly so partial results are never silently incomplete.
        unreachable: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Error {
        ok: bool,
        error: IpcErrorCode,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    InboundEvent {
        event: &'static str,
        from: String,
        envelope: Envelope,
    },
    RequestEvent {
        event: &'static str,
        request_id: Uuid,
        from: String,
        envelope: Envelope,
    },
    PeerCandidateEvent {
        event: &'static str,
        agent_id: String,
        public_key: String,
        locators: Vec<String>,
        source: &'static str,
    },
}

impl DaemonReply {
    /// The echoed request id, if any. Used to preserve correlation when a
    /// reply must be replaced — e.g. by a `message_too_large` error after
    /// the original reply exceeded the line limit.
    pub fn req_id(&self) -> Option<&str> {
        match self {
            Self::SendOk { req_id, .. }
            | Self::Peers { req_id, .. }
            | Self::Status { req_id, .. }
            | Self::Whoami { req_id, .. }
            | Self::PeerChanged { req_id, .. }
            | Self::Serving { req_id, .. }
            | Self::Replied { req_id, .. }
            | Self::WhoCan { req_id, .. }
            | Self::Error { req_id, .. } => req_id.as_deref(),
            Self::InboundEvent { .. }
            | Self::RequestEvent { .. }
            | Self::PeerCandidateEvent { .. } => None,
        }
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
