mod auth;
mod client_handler;
mod protocol;
mod server;

#[cfg(test)]
#[path = "client_handler_tests.rs"]
mod client_handler_tests;

pub use protocol::{
    CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcReplyKind, IpcSendKind,
    MAX_IPC_LINE_LENGTH, PeerSummary, ServiceMatch, ServiceSummary, WhoamiInfo, encode_reply_line,
    error_reply_line,
};
pub use server::{IpcServer, IpcServerConfig};
