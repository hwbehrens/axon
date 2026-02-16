use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::message::{Envelope, MessageKind};

// ---------------------------------------------------------------------------
// Known message kinds validation
// ---------------------------------------------------------------------------

pub fn validate_raw_kinds(raw: &[String]) -> Result<Vec<MessageKind>, String> {
    let mut result = Vec::with_capacity(raw.len());
    for s in raw {
        let kind: MessageKind = serde_json::from_value(serde_json::Value::String(s.clone()))
            .map_err(|e| e.to_string())?;
        if kind == MessageKind::Unknown {
            return Err(format!("unknown message kind: {s}"));
        }
        result.push(kind);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// IPC protocol types
// ---------------------------------------------------------------------------

/// Client-to-daemon IPC command, deserialized from line-delimited JSON.
/// Tagged by the `cmd` field (e.g., `{"cmd": "send", ...}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum IpcCommand {
    // v1 commands
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
    // v2 commands
    Hello {
        version: u32,
        #[serde(default)]
        req_id: Option<String>,
        #[serde(default = "default_consumer")]
        consumer: String,
    },
    Auth {
        token: String,
        #[serde(default)]
        req_id: Option<String>,
    },
    Whoami {
        #[serde(default)]
        req_id: Option<String>,
    },
    Inbox {
        #[serde(default = "default_inbox_limit")]
        limit: usize,
        #[serde(default)]
        kinds: Option<Vec<String>>,
        #[serde(default)]
        req_id: Option<String>,
    },
    Ack {
        up_to_seq: u64,
        #[serde(default)]
        req_id: Option<String>,
    },
    Subscribe {
        #[serde(default = "default_replay")]
        replay: bool,
        #[serde(default)]
        kinds: Option<Vec<String>>,
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
            | IpcCommand::Hello { req_id, .. }
            | IpcCommand::Auth { req_id, .. }
            | IpcCommand::Whoami { req_id, .. }
            | IpcCommand::Inbox { req_id, .. }
            | IpcCommand::Ack { req_id, .. }
            | IpcCommand::Subscribe { req_id, .. } => req_id.as_deref(),
        }
    }

    pub fn cmd_name(&self) -> &'static str {
        match self {
            IpcCommand::Send { .. } => "send",
            IpcCommand::Peers { .. } => "peers",
            IpcCommand::Status { .. } => "status",
            IpcCommand::Hello { .. } => "hello",
            IpcCommand::Auth { .. } => "auth",
            IpcCommand::Whoami { .. } => "whoami",
            IpcCommand::Inbox { .. } => "inbox",
            IpcCommand::Ack { .. } => "ack",
            IpcCommand::Subscribe { .. } => "subscribe",
        }
    }
}

fn default_inbox_limit() -> usize {
    50
}

fn default_consumer() -> String {
    "default".to_string()
}

fn default_replay() -> bool {
    true
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

/// A message stored in the IPC receive buffer, with daemon-assigned sequence
/// number and buffering timestamp.
#[derive(Debug, Clone, Serialize)]
pub struct BufferedMessage {
    pub seq: u64,
    pub buffered_at_ms: u64,
    pub envelope: Envelope,
}

/// Daemon identity information returned by the `whoami` command.
#[derive(Debug, Clone, Serialize)]
pub struct WhoamiInfo {
    pub agent_id: String,
    pub public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub ipc_version: u32,
    pub uptime_secs: u64,
}

// ---------------------------------------------------------------------------
// Error codes (IPC.md §5)
// ---------------------------------------------------------------------------

/// IPC error codes returned in error responses (IPC.md §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    HelloRequired,
    UnsupportedVersion,
    AuthRequired,
    AuthFailed,
    InvalidCommand,
    AckOutOfRange,
    PeerNotFound,
    PeerUnreachable,
    InternalError,
}

impl std::fmt::Display for IpcErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcErrorCode::HelloRequired => write!(f, "hello_required"),
            IpcErrorCode::UnsupportedVersion => write!(f, "unsupported_version"),
            IpcErrorCode::AuthRequired => write!(f, "auth_required"),
            IpcErrorCode::AuthFailed => write!(f, "auth_failed"),
            IpcErrorCode::InvalidCommand => write!(f, "invalid_command"),
            IpcErrorCode::AckOutOfRange => write!(f, "ack_out_of_range"),
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
            IpcErrorCode::HelloRequired => "v2 command sent without prior hello handshake",
            IpcErrorCode::UnsupportedVersion => "hello negotiated an unsupported protocol version",
            IpcErrorCode::AuthRequired => "command requires authentication",
            IpcErrorCode::AuthFailed => "invalid token or unauthorized",
            IpcErrorCode::InvalidCommand => {
                "malformed command, unknown cmd, or invalid field value"
            }
            IpcErrorCode::AckOutOfRange => "up_to_seq exceeds highest delivered sequence",
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
    // v1 replies
    SendAck {
        ok: bool,
        msg_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
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
    // Legacy v1 inbound push (for v1 clients)
    Inbound {
        inbound: bool,
        envelope: Envelope,
    },
    // v2 pushed inbound event (§3.5, §3.6) — no ok, no req_id
    InboundEvent {
        event: &'static str, // always "inbound"
        replay: bool,
        seq: u64,
        buffered_at_ms: u64,
        envelope: Envelope,
    },
    // v2 replies
    Hello {
        ok: bool,
        version: u32,
        daemon_max_version: u32,
        agent_id: String,
        features: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Auth {
        ok: bool,
        auth: String,
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
    Inbox {
        ok: bool,
        messages: Vec<BufferedMessage>,
        next_seq: Option<u64>,
        has_more: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Ack {
        ok: bool,
        acked_seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
    Subscribe {
        ok: bool,
        subscribed: bool,
        replayed: usize,
        replay_to_seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
    },
}

#[cfg(test)]
#[path = "protocol_tests/mod.rs"]
mod tests;
