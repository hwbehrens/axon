//! Adversarial and stress tests.
//!
//! These tests exercise the system under hostile inputs, concurrent
//! contention, and boundary conditions to verify resilience.

use std::time::Duration;

use axon::config::{Config, KnownPeer, load_known_peers, save_known_peers};
use axon::ipc::{DaemonReply, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MAX_MESSAGE_SIZE, MessageKind, PROTOCOL_VERSION, decode, encode};
use axon::peer_table::{ConnectionStatus, PeerTable};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

mod adversarial {
    pub(crate) mod ipc;
    pub(crate) mod peer_table;
    pub(crate) mod validation;
}

// =========================================================================
// Helpers
// =========================================================================

pub(crate) fn agent_a() -> String {
    "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8".to_string()
}

pub(crate) fn agent_b() -> String {
    "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

pub(crate) fn random_agent_ids(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("ed25519.{:0>32x}", i)).collect()
}
