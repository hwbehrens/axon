//! Inbound stream handlers for established QUIC connections: unidirectional
//! fire-and-forget intake and bidirectional request/response dispatch.
//!
//! Split from `connection.rs` for file-length limits. This is a child module,
//! so the items below retain access to the framing helpers declared there.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::broadcast;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::message::{AgentId, Envelope, MessageKind};

use super::super::quic_transport::ResponseHandlerFn;
use super::{default_error_response, overwrite_authenticated_identity, read_framed, write_framed};

// ---------------------------------------------------------------------------
// Connection context — shared state for stream handlers
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(super) struct ConnectionContext {
    pub(super) local_agent_id: AgentId,
    pub(super) inbound_tx: broadcast::Sender<Arc<Envelope>>,
    pub(super) response_handler: Option<ResponseHandlerFn>,
    pub(super) inbound_read_timeout: Duration,
}

// ---------------------------------------------------------------------------
// Unidirectional stream handler
// ---------------------------------------------------------------------------

pub(super) async fn handle_uni_stream(
    ctx: &ConnectionContext,
    peer_id: &str,
    mut recv: quinn::RecvStream,
) {
    match timeout(ctx.inbound_read_timeout, read_framed(&mut recv)).await {
        Ok(Ok(bytes)) => match serde_json::from_slice::<Envelope>(&bytes) {
            Ok(mut envelope) => {
                overwrite_authenticated_identity(&mut envelope, peer_id, &ctx.local_agent_id);
                if !envelope.kind.is_allowed_on_unidirectional() {
                    debug!(kind = %envelope.kind, "dropping kind that is invalid on uni stream");
                } else if let Err(err) = envelope.validate() {
                    debug!(error = %err, "dropping invalid uni envelope");
                } else {
                    let _ = ctx.inbound_tx.send(Arc::new(envelope));
                }
            }
            Err(err) => {
                debug!(error = %err, "dropping malformed uni envelope");
            }
        },
        Ok(Err(err)) => {
            warn!(error = %err, peer = peer_id, "failed reading uni stream");
        }
        Err(_) => {
            warn!(peer = peer_id, "uni stream read timed out");
        }
    }
}

// ---------------------------------------------------------------------------
// Bidirectional stream handler
// ---------------------------------------------------------------------------

/// Handle an authenticated bidi request.
pub(super) async fn handle_authenticated_bidi(
    ctx: &ConnectionContext,
    request: Envelope,
    mut send: quinn::SendStream,
) {
    if let Err(err) = request.validate() {
        let response = Envelope::response_to(
            &request,
            ctx.local_agent_id.clone(),
            MessageKind::Error,
            json!({
                "code": "invalid_envelope",
                "message": format!("envelope validation failed: {err}"),
                "retryable": false,
            }),
        );
        send_response(&mut send, &response).await;
    } else if let MessageKind::Unknown(kind) = &request.kind {
        let response = Envelope::response_to(
            &request,
            ctx.local_agent_id.clone(),
            MessageKind::Error,
            json!({
                "code": "unsupported_kind",
                "message": format!(
                    "unsupported message kind '{}' on bidirectional stream",
                    kind.chars().take(64).collect::<String>()
                ),
                "retryable": false,
            }),
        );
        send_response(&mut send, &response).await;
    } else if !request.kind.expects_response() {
        let response = Envelope::response_to(
            &request,
            ctx.local_agent_id.clone(),
            MessageKind::Error,
            json!({
                "code": "invalid_envelope",
                "message": format!("message kind '{}' cannot initiate a bidirectional stream", request.kind),
                "retryable": false,
            }),
        );
        send_response(&mut send, &response).await;
    } else {
        let request_arc = Arc::new(request.clone());
        let response = if let Some(ref handler) = ctx.response_handler {
            match handler(request_arc).await {
                Some(resp) => resp,
                None => default_error_response(&request, ctx.local_agent_id.as_str()),
            }
        } else {
            default_error_response(&request, ctx.local_agent_id.as_str())
        };
        send_response(&mut send, &response).await;
    }
}

pub(super) async fn handle_bidi_stream(
    ctx: &ConnectionContext,
    peer_id: &str,
    send: quinn::SendStream,
    mut recv: quinn::RecvStream,
) {
    let mut request = match timeout(ctx.inbound_read_timeout, read_framed(&mut recv)).await {
        Ok(Ok(bytes)) => match serde_json::from_slice::<Envelope>(&bytes) {
            Ok(r) => r,
            Err(err) => {
                debug!(error = %err, "dropping malformed bidi envelope");
                return;
            }
        },
        Ok(Err(err)) => {
            warn!(error = %err, peer = peer_id, "failed reading bidi stream");
            return;
        }
        Err(_) => {
            warn!(peer = peer_id, "bidi stream read timed out");
            return;
        }
    };

    overwrite_authenticated_identity(&mut request, peer_id, &ctx.local_agent_id);
    handle_authenticated_bidi(ctx, request, send).await;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_response(send: &mut quinn::SendStream, response: &Envelope) {
    if let Ok(response_bytes) = response.wire_encode()
        && write_framed(send, &response_bytes).await.is_ok()
    {
        let _ = send.finish();
    }
}
