mod auth;
mod bind;
mod bounds;
mod client_handler;
mod protocol;
mod server;

#[cfg(test)]
#[path = "client_handler_tests.rs"]
mod client_handler_tests;

pub use bounds::{bounded_peers_reply, bounded_who_can_reply};
pub use protocol::{
    CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcReplyKind, IpcSendKind,
    MAX_IPC_LINE_LENGTH, PeerSummary, ServiceMatch, ServiceSummary, WhoamiInfo, encode_reply_line,
    error_reply_line,
};
pub use server::{IpcServer, IpcServerConfig};
