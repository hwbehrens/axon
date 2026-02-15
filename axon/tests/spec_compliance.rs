//! Wire protocol compliance tests.
//!
//! Each test verifies a specific requirement from `spec/SPEC.md` or
//! `spec/MESSAGE_TYPES.md` (v2). Tests are grouped by spec section.

use axon::message::*;
use serde_json::{Value, json};

#[path = "spec_compliance/envelope.rs"]
mod envelope;
#[path = "spec_compliance/ipc_v2.rs"]
mod ipc_v2;
#[path = "spec_compliance/payloads.rs"]
mod payloads;
#[path = "spec_compliance/stream_mapping.rs"]
mod stream_mapping;
#[path = "spec_compliance/wire_format.rs"]
mod wire_format;

// =========================================================================
// Helpers
// =========================================================================

pub(crate) fn agent_a() -> String {
    "ed25519.a1b2c3d4e5f6a7b8a1b2c3d4e5f6a7b8".to_string()
}

pub(crate) fn agent_b() -> String {
    "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

pub(crate) fn to_json(env: &Envelope) -> Value {
    serde_json::to_value(env).unwrap()
}
