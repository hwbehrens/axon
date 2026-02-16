use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::message::{Envelope, MessageKind};

// ---------------------------------------------------------------------------
// IPC protocol types
// ---------------------------------------------------------------------------

/// Client-to-daemon IPC command, deserialized from line-delimited JSON.
/// Tagged by the `cmd` field (e.g., `{"cmd": "send", ...}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum IpcCommand {
    Send {
        to: String,
        kind: MessageKind,
        payload: Value,
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
}

impl IpcCommand {
    pub fn req_id(&self) -> Option<&str> {
        match self {
            IpcCommand::Send { req_id, .. }
            | IpcCommand::Peers { req_id, .. }
            | IpcCommand::Status { req_id, .. }
            | IpcCommand::Whoami { req_id, .. } => req_id.as_deref(),
        }
    }

    pub fn cmd_name(&self) -> &'static str {
        match self {
            IpcCommand::Send { .. } => "send",
            IpcCommand::Peers { .. } => "peers",
            IpcCommand::Status { .. } => "status",
            IpcCommand::Whoami { .. } => "whoami",
        }
    }
}

/// A parsed IPC command paired with the originating client's connection ID.
#[derive(Debug, Clone)]
pub struct CommandEvent {
    pub client_id: u64,
    pub command: IpcCommand,
}

/// Summary of a connected or known peer, returned by the `peers` command.
#[derive(Debug, Clone, Serialize)]
pub struct PeerSummary {
    pub id: String,
    pub addr: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    pub source: String,
}

/// Daemon identity information returned by the `whoami` command.
#[derive(Debug, Clone, Serialize)]
pub struct WhoamiInfo {
    pub agent_id: String,
    pub public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub uptime_secs: u64,
}

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

/// IPC error codes returned in error responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    InvalidCommand,
    PeerNotFound,
    PeerUnreachable,
    InternalError,
}

impl std::fmt::Display for IpcErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcErrorCode::InvalidCommand => write!(f, "invalid_command"),
            IpcErrorCode::PeerNotFound => write!(f, "peer_not_found"),
            IpcErrorCode::PeerUnreachable => write!(f, "peer_unreachable"),
            IpcErrorCode::InternalError => write!(f, "internal_error"),
        }
    }
}

impl IpcErrorCode {
    /// Human-readable explanation of the error code.
    pub fn message(&self) -> &'static str {
        match self {
            IpcErrorCode::InvalidCommand => {
                "malformed command, unknown cmd, or invalid field value"
            }
            IpcErrorCode::PeerNotFound => "target agent_id not in peer table",
            IpcErrorCode::PeerUnreachable => "peer known but connection failed or timed out",
            IpcErrorCode::InternalError => "unexpected daemon error",
        }
    }
}

// ---------------------------------------------------------------------------
// Daemon replies
// ---------------------------------------------------------------------------

/// Daemon-to-client IPC response, serialized as line-delimited JSON.
/// Uses `#[serde(untagged)]` — variants are distinguished by their field shapes.
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
    Error {
        ok: bool,
        error: IpcErrorCode,
        message: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    InboundEvent {
        event: &'static str, // always "inbound"
        from: String,
        envelope: Envelope,
    },
    Whoami {
        ok: bool,
        #[serde(flatten)]
        info: WhoamiInfo,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
}
