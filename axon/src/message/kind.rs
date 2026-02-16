use std::fmt;

use serde::{Deserialize, Serialize};

/// AXON message kind — determines stream mapping.
///
/// - `Request` → bidirectional stream (expects a `Response` or `Error`)
/// - `Response` → bidirectional stream (reply to a `Request`)
/// - `Message` → unidirectional stream (fire-and-forget)
/// - `Error` → bidirectional stream (error reply to a `Request`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Request,
    Response,
    Message,
    Error,
    #[serde(other)]
    Unknown,
}

impl MessageKind {
    pub fn expects_response(self) -> bool {
        matches!(self, MessageKind::Request)
    }

    pub fn is_response(self) -> bool {
        matches!(self, MessageKind::Response | MessageKind::Error)
    }
}

impl fmt::Display for MessageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            MessageKind::Request => "request",
            MessageKind::Response => "response",
            MessageKind::Message => "message",
            MessageKind::Error => "error",
            MessageKind::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
#[path = "kind_tests.rs"]
mod tests;
