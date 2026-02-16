mod broadcast;
mod commands;
mod dispatch;
mod hello_auth;

use std::collections::HashMap;
use std::sync::Arc;

use super::protocol::DaemonReply;
use super::receive_buffer::ReceiveBuffer;
use crate::message::{Envelope, MessageKind};
use tokio::sync::Mutex;

const IPC_VERSION: u32 = 2;
const MAX_CONSUMER_LEN: usize = 64;

/// Result of dispatching an IPC command through the unified handler.
pub struct DispatchResult {
    pub reply: DaemonReply,
    pub response_envelope: Option<Envelope>,
    /// If true, the daemon should close this client after sending the reply.
    pub close: bool,
}

// ---------------------------------------------------------------------------
// Client state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SubscriptionFilter {
    pub kinds: Option<Vec<MessageKind>>,
    pub replay_to_seq: u64,
}

impl SubscriptionFilter {
    pub fn matches(&self, kind: &MessageKind) -> bool {
        match &self.kinds {
            None => true,
            Some(kinds) => kinds.contains(kind),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClientState {
    pub version: Option<u32>,
    pub authenticated: bool,
    pub subscription: Option<SubscriptionFilter>,
    pub consumer: String,
}

impl ClientState {
    /// Returns true if the client has completed the hello handshake.
    fn has_hello(&self) -> bool {
        self.version.is_some()
    }

    /// Returns the negotiated version, defaulting to 1 for pre-hello clients.
    fn negotiated_version(&self) -> u32 {
        self.version.unwrap_or(1)
    }

    /// Returns true if the client negotiated v2+ semantics.
    fn is_v2_semantics(&self) -> bool {
        self.negotiated_version() >= 2
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

pub struct IpcServerConfig {
    pub agent_id: String,
    pub public_key: String,
    pub name: Option<String>,
    pub version: String,
    pub token: Option<String>,
    pub buffer_size: usize,
    pub buffer_ttl_secs: u64,
    pub buffer_byte_cap: Option<usize>,
    pub allow_v1: bool,
    pub uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for IpcServerConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            public_key: String::new(),
            name: None,
            version: "0.1.0".to_string(),
            token: None,
            buffer_size: 1000,
            buffer_ttl_secs: 86400,
            buffer_byte_cap: None,
            allow_v1: true,
            uptime_secs: Arc::new(|| 0),
            clock: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// IPC command handlers
// ---------------------------------------------------------------------------

pub struct IpcHandlers {
    config: Arc<IpcServerConfig>,
    client_states: Arc<Mutex<HashMap<u64, ClientState>>>,
    receive_buffer: Arc<Mutex<ReceiveBuffer>>,
    clients: Arc<Mutex<HashMap<u64, tokio::sync::mpsc::Sender<Arc<str>>>>>,
}

impl IpcHandlers {
    pub fn new(
        config: Arc<IpcServerConfig>,
        client_states: Arc<Mutex<HashMap<u64, ClientState>>>,
        receive_buffer: Arc<Mutex<ReceiveBuffer>>,
        clients: Arc<Mutex<HashMap<u64, tokio::sync::mpsc::Sender<Arc<str>>>>>,
    ) -> Self {
        Self {
            config,
            client_states,
            receive_buffer,
            clients,
        }
    }
}
