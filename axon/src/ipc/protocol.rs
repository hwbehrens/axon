use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::message::{AgentId, Envelope, MessageKind};

pub const MAX_IPC_LINE_LENGTH: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpcSendKind {
    Request,
    Message,
}

impl IpcSendKind {
    pub fn as_message_kind(self) -> MessageKind {
        match self {
            Self::Request => MessageKind::Request,
            Self::Message => MessageKind::Message,
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
    Serve {
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
            | Self::Serve { req_id }
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

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
