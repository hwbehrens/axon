//! Typed failure classes for inbound request-event delivery.
//!
//! The daemon must distinguish a handler that is actually gone (or was
//! overflow-disconnected) from a healthy handler whose event could not be
//! framed. Conflating the two let one wire-legal near-limit request revoke
//! a live handler's lease (spec/IPC.md §2, §6.2, §7).

use crate::ipc::protocol::{DaemonReply, EncodeLineError, encode_reply_line};
use crate::message::Envelope;

use super::IpcServer;

/// Why delivering an inbound request event to the handler failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestEventError {
    /// The encoded event exceeds the IPC line limit. The client is still
    /// connected; only the affected request fails.
    TooLarge(usize),
    /// Serialization failed unexpectedly; the client is unaffected.
    Serialize(String),
    /// The handler client is no longer connected; its lease must be released.
    Disconnected,
    /// The handler's bounded queue overflowed; the client was closed and its
    /// lease must be released (spec/IPC.md §7).
    QueueOverflow,
}

impl std::fmt::Display for RequestEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge(bytes) => write!(
                f,
                "IPC request event exceeds the line limit ({bytes} bytes)"
            ),
            Self::Serialize(err) => write!(f, "failed to serialize request event: {err}"),
            Self::Disconnected => write!(f, "IPC request handler disconnected"),
            Self::QueueOverflow => write!(f, "IPC request handler queue overflowed"),
        }
    }
}

impl std::error::Error for RequestEventError {}

impl IpcServer {
    /// Deliver an inbound request event, preserving the failure class for
    /// the caller. An oversized event is not a disconnect or a queue
    /// overflow: it fails delivery explicitly and only the daemon's response
    /// handler decides whether the handler's lease survives (spec/IPC.md
    /// §2, §6.2). [`Self::send_request_event`] is the historical
    /// `anyhow::Result` wrapper.
    pub async fn try_send_request_event(
        &self,
        client_id: u64,
        envelope: &Envelope,
    ) -> std::result::Result<(), RequestEventError> {
        let event = DaemonReply::RequestEvent {
            event: "request",
            request_id: envelope.id,
            from: envelope
                .from
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            envelope: envelope.clone(),
        };
        let line = match encode_reply_line(&event) {
            Ok(line) => line,
            Err(EncodeLineError::TooLarge(bytes)) => {
                return Err(RequestEventError::TooLarge(bytes));
            }
            Err(EncodeLineError::Serialize(err)) => {
                return Err(RequestEventError::Serialize(err));
            }
        };
        let client = self.clients.lock().await.get(&client_id).cloned();
        let Some(client) = client else {
            return Err(RequestEventError::Disconnected);
        };
        if client.tx.try_send(line).is_err() {
            self.close_client(client_id).await;
            return Err(RequestEventError::QueueOverflow);
        }
        Ok(())
    }

    /// Historical entry point kept source-compatible; the daemon uses
    /// [`Self::try_send_request_event`] to distinguish gone/overflow-closed
    /// handlers from a healthy handler whose event could not be framed.
    pub async fn send_request_event(
        &self,
        client_id: u64,
        envelope: &Envelope,
    ) -> anyhow::Result<()> {
        self.try_send_request_event(client_id, envelope)
            .await
            .map_err(anyhow::Error::new)
    }
}
