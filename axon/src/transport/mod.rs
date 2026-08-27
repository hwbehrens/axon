mod connection;
mod connection_registry;
mod quic_transport;
mod reconnect;
mod tls;

use std::time::Duration;

use crate::message::{AgentId, MAX_MESSAGE_SIZE};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time a single outbound QUIC dial (handshake included) may take.
pub(crate) const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const MAX_MESSAGE_SIZE_USIZE: usize = MAX_MESSAGE_SIZE as usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialPeer {
    pub agent_id: AgentId,
    pub addr: std::net::SocketAddr,
}

#[derive(Debug, Clone)]
pub struct PairRequest {
    pub agent_id: String,
    pub pubkey: String,
    pub addr: Option<String>,
}

pub use connection::{SendError, default_error_response};
pub use quic_transport::{ConnectionManager, ResponseHandlerFn};
pub use tls::extract_ed25519_pubkey_from_cert_der;
