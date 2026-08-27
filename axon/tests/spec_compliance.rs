//! Wire protocol compliance tests.
//!
//! Each test verifies a specific requirement from `spec/SPEC.md` or
//! `spec/MESSAGE_TYPES.md` (v2). Tests are grouped by spec section.

use axon::message::*;
use serde_json::{Value, json};

#[path = "spec_compliance/cli_help.rs"]
mod cli_help;
#[path = "spec_compliance/envelope.rs"]
mod envelope;
#[path = "spec_compliance/ipc_errors.rs"]
mod ipc_errors;
#[path = "spec_compliance/payloads.rs"]
mod payloads;
#[path = "spec_compliance/stream_mapping.rs"]
mod stream_mapping;
#[path = "spec_compliance/wire_format/mod.rs"]
mod wire_format;

// =========================================================================
// Helpers
// =========================================================================

pub(crate) fn agent_a() -> AgentId {
    AgentId::parse("ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8").unwrap()
}

pub(crate) fn agent_b() -> AgentId {
    AgentId::parse("ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3").unwrap()
}

pub(crate) fn to_json(env: &Envelope) -> Value {
    serde_json::to_value(env).unwrap()
}
