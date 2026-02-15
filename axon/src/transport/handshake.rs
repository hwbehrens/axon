use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

use crate::message::{
    AckPayload, CapabilitiesPayload, Envelope, ErrorCode, ErrorPayload, HelloPayload, MessageKind,
    PeerStatus, PongPayload, ResponsePayload, hello_features,
};

use super::tls::derive_agent_id_from_pubkey_bytes;

pub fn auto_response(request: &Envelope, local_agent_id: &str) -> Envelope {
    match request.kind {
        MessageKind::Hello => {
            if !hello_request_supports_protocol_v1(request) {
                return Envelope::response_to(
                    request,
                    local_agent_id.to_string(),
                    MessageKind::Error,
                    serde_json::to_value(ErrorPayload {
                        code: ErrorCode::IncompatibleVersion,
                        message: format!(
                            "no mutually supported protocol version. This agent supports: [1]. \
                             Received: {:?}",
                            request
                                .payload_value()
                                .unwrap_or_default()
                                .get("protocol_versions")
                        ),
                        retryable: false,
                    })
                    .unwrap(),
                );
            }

            let payload = serde_json::to_value(HelloPayload {
                protocol_versions: vec![1],
                selected_version: Some(1),
                agent_name: None,
                features: hello_features(),
            })
            .unwrap();
            Envelope::response_to(
                request,
                local_agent_id.to_string(),
                MessageKind::Hello,
                payload,
            )
        }
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

pub(crate) fn validate_hello_identity(
    hello: &Envelope,
    cert_pubkey_b64: &str,
    expected_pubkeys: &Arc<StdRwLock<HashMap<String, String>>>,
) -> std::result::Result<(), String> {
    let cert_pubkey_bytes = STANDARD
        .decode(cert_pubkey_b64)
        .map_err(|_| "peer certificate key was not valid base64".to_string())?;
    let derived_agent_id = derive_agent_id_from_pubkey_bytes(&cert_pubkey_bytes);
    if hello.from != derived_agent_id {
        return Err("peer hello 'from' does not match certificate public key identity".to_string());
    }

    let expected = expected_pubkeys
        .read()
        .map_err(|_| "expected peer table lock poisoned".to_string())?;
    if let Some(expected_pubkey) = expected.get(hello.from.as_str())
        && expected_pubkey != cert_pubkey_b64
    {
        return Err("peer certificate public key does not match discovered key".to_string());
    }

    Ok(())
}

pub(crate) fn hello_request_supports_protocol_v1(hello: &Envelope) -> bool {
    let payload = hello.payload_value().unwrap_or_default();
    payload
        .get("protocol_versions")
        .and_then(|v| v.as_array())
        .map(|versions| versions.iter().any(|v| v.as_u64() == Some(1)))
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod tests;
