mod connection;
mod framing;
mod handshake;
mod quic_transport;
mod tls;

use std::time::Duration;

use crate::message::MAX_MESSAGE_SIZE;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const MAX_MESSAGE_SIZE_USIZE: usize = MAX_MESSAGE_SIZE as usize;

pub use handshake::auto_response;
pub use quic_transport::{QuicTransport, ResponseHandlerFn};
pub use tls::extract_ed25519_pubkey_from_cert_der;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
