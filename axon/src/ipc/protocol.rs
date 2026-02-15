use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::message::{Envelope, MessageKind};

// ---------------------------------------------------------------------------
// IPC protocol types
// ---------------------------------------------------------------------------

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
    },
    Peers,
    Status,
    // v2 commands
    Hello {
        version: u32,
    },
    Auth {
        token: String,
    },
    Whoami,
    Inbox {
        #[serde(default = "default_inbox_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<String>,
        #[serde(default)]
        kinds: Option<Vec<MessageKind>>,
    },
    Ack {
        ids: Vec<Uuid>,
    },
    Subscribe {
        #[serde(default)]
        since: Option<String>,
        #[serde(default)]
        kinds: Option<Vec<MessageKind>>,
    },
}

fn default_inbox_limit() -> usize {
    50
}

#[derive(Debug, Clone)]
pub struct CommandEvent {
    pub client_id: u64,
    pub command: IpcCommand,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerSummary {
    pub id: String,
    pub addr: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BufferedMessage {
    pub envelope: Envelope,
    pub buffered_at: String, // ISO 8601 timestamp
}

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

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DaemonReply {
    // v1 replies
    SendAck {
        ok: bool,
        msg_id: Uuid,
    },
    Peers {
        ok: bool,
        peers: Vec<PeerSummary>,
    },
    Status {
        ok: bool,
        uptime_secs: u64,
        peers_connected: usize,
        messages_sent: u64,
        messages_received: u64,
    },
    Error {
        ok: bool,
        error: String,
    },
    Inbound {
        inbound: bool,
        envelope: Envelope,
    },
    // v2 replies
    Hello {
        ok: bool,
        version: u32,
        agent_id: String,
        features: Vec<String>,
    },
    Auth {
        ok: bool,
        auth: String, // "accepted" or error message
    },
    Whoami {
        ok: bool,
        #[serde(flatten)]
        info: WhoamiInfo,
    },
    Inbox {
        ok: bool,
        messages: Vec<BufferedMessage>,
        has_more: bool,
    },
    Ack {
        ok: bool,
        acked: usize,
    },
    Subscribe {
        ok: bool,
        subscribed: bool,
        replayed: usize,
    },
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
