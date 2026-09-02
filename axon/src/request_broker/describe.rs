//! Inbound `describe` answering.
//!
//! The broker answers `describe` requests itself from the manifest its
//! handler published at `serve` time — the handler is never woken, and the
//! response never enters the completed-response tombstone cache (describe is
//! side-effect free, so a fresh answer is always correct across manifest
//! refreshes). Manifests are claims: this path never reads or mutates trust
//! state.

use crate::manifest::Manifest;
use crate::message::{Envelope, MAX_MESSAGE_SIZE, MessageKind};
use serde_json::json;

use super::RequestBroker;

impl RequestBroker {
    /// Answer an inbound `describe` request from the registered manifest, or
    /// with an instructive `no_manifest` error.
    pub(super) fn describe_response(
        &self,
        manifest: Option<&Manifest>,
        request: &Envelope,
    ) -> Envelope {
        let Some(manifest) = manifest else {
            return self.error_response(
                request,
                "no_manifest",
                "no capability manifest registered; the application handler has not published one (serve with a manifest to publish it)",
                false,
            );
        };
        let payload = manifest
            .to_payload_value()
            .unwrap_or_else(|_| json!({"services": []}));
        let response = Envelope::response_to(
            request,
            self.local_agent_id.clone(),
            MessageKind::Response,
            payload,
        );
        // Serve-time validation bounds manifests at 32 KiB, so this cannot
        // trigger in practice; the defensive fallback keeps an unsendable
        // response from ever reaching the transport.
        match response.wire_encode() {
            Ok(bytes) if bytes.len() <= MAX_MESSAGE_SIZE as usize => response,
            _ => self.error_response(
                request,
                "manifest_too_large",
                "registered capability manifest exceeds the wire limit",
                false,
            ),
        }
    }
}

#[cfg(test)]
#[path = "describe_tests.rs"]
mod tests;
