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
    Send {
        to: String,
        kind: MessageKind,
        payload: Value,
        #[serde(default, rename = "ref")]
        ref_id: Option<Uuid>,
    },
    Peers,
    Status,
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
#[serde(untagged)]
pub enum DaemonReply {
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
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
