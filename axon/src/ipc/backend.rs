use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

use super::protocol::PeerSummary;
use crate::message::MessageKind;

/// Backend trait for IPC command effects that require daemon-level resources
/// (transport, peer table, counters). IPC handlers own policy (auth, hello,
/// req_id gating); the backend owns effects (sending messages, listing peers,
/// reporting status).
#[allow(async_fn_in_trait)]
pub trait IpcBackend: Send + Sync {
    /// Send a message to a peer. Returns the message UUID on success,
    /// along with an optional response envelope that should be broadcast.
    async fn send_message(
        &self,
        to: String,
        kind: MessageKind,
        payload: Value,
        ref_id: Option<Uuid>,
    ) -> Result<SendResult>;

    /// List all known peers.
    async fn peers(&self) -> Result<Vec<PeerSummary>>;

    /// Get daemon status: (uptime_secs, peers_connected, messages_sent, messages_received).
    async fn status(&self) -> Result<StatusResult>;
}

pub struct SendResult {
    pub msg_id: Uuid,
    pub response: Option<crate::message::Envelope>,
}

pub struct StatusResult {
    pub uptime_secs: u64,
    pub peers_connected: usize,
    pub messages_sent: u64,
    pub messages_received: u64,
}
