//! Adversarial and stress tests.
//!
//! These tests exercise the system under hostile inputs, concurrent
//! contention, and boundary conditions to verify resilience.

use std::time::Duration;

use axon::config::Config;
use axon::ipc::{DaemonReply, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MAX_MESSAGE_SIZE, MessageKind, decode, encode};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

mod adversarial {
    pub(crate) mod ipc;
    pub(crate) mod ipc_framing;
    pub(crate) mod validation;
}

// =========================================================================
// Helpers
// =========================================================================

pub(crate) fn agent_a() -> axon::message::AgentId {
    axon::message::AgentId::parse("ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8").unwrap()
}

pub(crate) fn agent_b() -> axon::message::AgentId {
    axon::message::AgentId::parse("ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3").unwrap()
}
