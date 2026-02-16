use std::fmt;

use serde::{Deserialize, Serialize};

/// AXON message kind — determines stream mapping and payload schema.
///
/// See `spec/MESSAGE_TYPES.md` §Core Types for the full kind table and
/// `spec/MESSAGE_TYPES.md` §Payload Schemas for per-kind payload definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Hello,
    Ping,
    Pong,
    Query,
    Response,
    Delegate,
    Ack,
    Result,
    Notify,
    Cancel,
    Discover,
    Capabilities,
    Error,
    #[serde(other)]
    Unknown,
}

impl MessageKind {
    pub fn expects_response(self) -> bool {
        matches!(
            self,
            MessageKind::Hello
                | MessageKind::Ping
                | MessageKind::Query
                | MessageKind::Delegate
                | MessageKind::Cancel
                | MessageKind::Discover
        )
    }

    pub fn is_response(self) -> bool {
        matches!(
            self,
            MessageKind::Pong
                | MessageKind::Response
                | MessageKind::Ack
                | MessageKind::Capabilities
                | MessageKind::Error
        )
    }

    pub fn is_required(self) -> bool {
        matches!(
            self,
            MessageKind::Hello
                | MessageKind::Ping
                | MessageKind::Pong
                | MessageKind::Query
                | MessageKind::Response
                | MessageKind::Notify
                | MessageKind::Error
        )
    }
}

impl fmt::Display for MessageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            MessageKind::Hello => "hello",
            MessageKind::Ping => "ping",
            MessageKind::Pong => "pong",
            MessageKind::Query => "query",
            MessageKind::Response => "response",
            MessageKind::Delegate => "delegate",
            MessageKind::Ack => "ack",
            MessageKind::Result => "result",
            MessageKind::Notify => "notify",
            MessageKind::Cancel => "cancel",
            MessageKind::Discover => "discover",
            MessageKind::Capabilities => "capabilities",
            MessageKind::Error => "error",
            MessageKind::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

pub fn hello_features() -> Vec<String> {
    vec![
        "delegate".to_string(),
        "ack".to_string(),
        "result".to_string(),
        "cancel".to_string(),
        "discover".to_string(),
        "capabilities".to_string(),
    ]
}

#[cfg(test)]
#[path = "kind_tests.rs"]
mod tests;
