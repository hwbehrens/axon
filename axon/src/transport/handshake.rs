use serde_json::json;

use crate::message::{Envelope, MessageKind};

/// Default response for unhandled bidi requests when no response handler is
/// registered (or the handler returns `None`).
pub fn default_error_response(request: &Envelope, local_agent_id: &str) -> Envelope {
    Envelope::response_to(
        request,
        local_agent_id.to_string(),
        MessageKind::Error,
        json!({
            "code": "unhandled",
            "message": format!(
                "no application handler registered for request '{}'",
                request.id
            ),
            "retryable": false,
        }),
    )
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod tests;
