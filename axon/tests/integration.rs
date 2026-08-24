//! Integration tests — cross-module interactions.
//!
//! These tests exercise multiple subsystems together without starting
//! a full daemon process.

use std::path::PathBuf;
use std::time::Duration;

use axon::config::AxonPaths;
use axon::identity::Identity;
use axon::ipc::{DaemonReply, IpcCommand, IpcServer, IpcServerConfig};
use axon::message::{AgentId, Envelope, MessageKind, decode, encode};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

mod integration {
    pub(crate) mod discovery;
    pub(crate) mod identity;
    pub(crate) mod ipc;
    pub(crate) mod transport;
}

// =========================================================================
// Helpers
// =========================================================================

pub(crate) fn make_identity() -> (Identity, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let id = Identity::load_or_generate(&paths).unwrap();
    (id, dir)
}
