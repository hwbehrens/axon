use serde_json::json;

use crate::message::{
    AckPayload, CapabilitiesPayload, Envelope, ErrorCode, ErrorPayload, MessageKind, PeerStatus,
    PongPayload, ResponsePayload,
};

pub fn auto_response(request: &Envelope, local_agent_id: &str) -> Envelope {
    match request.kind {
        MessageKind::Ping => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Pong,
            serde_json::to_value(PongPayload {
                status: PeerStatus::Idle,
                uptime_secs: 0,
                active_tasks: 0,
                agent_name: None,
            })
            .unwrap(),
        ),
        MessageKind::Discover => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Capabilities,
            serde_json::to_value(CapabilitiesPayload {
                agent_name: Some("AXON Agent".to_string()),
                domains: vec!["meta.status".to_string()],
                channels: vec![],
                tools: vec!["axon".to_string()],
                max_concurrent_tasks: Some(1),
                model: None,
            })
            .unwrap(),
        ),
        MessageKind::Query => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Response,
            serde_json::to_value(ResponsePayload {
                data: json!({"accepted": true}),
                summary: "Query received by AXON transport layer. No application handler is \
                          registered; connect an IPC client to process queries."
                    .to_string(),
                tokens_used: Some(0),
                truncated: Some(false),
            })
            .unwrap(),
        ),
        MessageKind::Delegate | MessageKind::Cancel => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Ack,
            serde_json::to_value(AckPayload {
                accepted: true,
                estimated_ms: None,
            })
            .unwrap(),
        ),
        _ => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Error,
            serde_json::to_value(ErrorPayload {
                code: ErrorCode::UnknownKind,
                message: format!(
                    "unsupported request kind '{}' on bidirectional stream. \
                     Supported request kinds: hello, ping, query, delegate, cancel, discover. \
                     Use a unidirectional stream for fire-and-forget kinds (notify, result).",
                    request.kind
                ),
                retryable: false,
            })
            .unwrap(),
        ),
    }
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod tests;
