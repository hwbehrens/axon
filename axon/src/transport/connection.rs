use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustls::pki_types::CertificateDer;
use tokio::sync::{RwLock, broadcast};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::message::{Envelope, ErrorCode, ErrorPayload, MessageKind};

use super::framing::{read_framed, write_framed};
use super::handshake::{auto_response, validate_hello_identity};
use super::quic_transport::{ReplayCheckFn, ResponseHandlerFn};
use super::tls::extract_ed25519_pubkey_from_cert_der;

pub(crate) fn extract_peer_pubkey_base64_from_connection(
    connection: &quinn::Connection,
) -> Result<String> {
    let identity = connection
        .peer_identity()
        .ok_or_else(|| anyhow!("peer did not provide an identity"))?;
    let certs = identity
        .downcast::<Vec<CertificateDer>>()
        .map_err(|_| anyhow!("peer identity was not a rustls certificate chain"))?;

    let cert = certs
        .first()
        .ok_or_else(|| anyhow!("peer certificate chain is empty"))?;

    let key = extract_ed25519_pubkey_from_cert_der(cert.as_ref())?;
    Ok(STANDARD.encode(key))
}

// ---------------------------------------------------------------------------
// Connection context — shared state for stream handlers
// ---------------------------------------------------------------------------

struct ConnectionContext {
    connection: quinn::Connection,
    local_agent_id: String,
    peer_cert_pubkey_b64: String,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
    connections: Arc<RwLock<HashMap<String, quinn::Connection>>>,
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
    replay_check: Option<ReplayCheckFn>,
    response_handler: Option<ResponseHandlerFn>,
    inbound_read_timeout: Duration,
}

// ---------------------------------------------------------------------------
// Unidirectional stream handler
// ---------------------------------------------------------------------------

async fn handle_uni_stream(
    ctx: &ConnectionContext,
    authenticated_peer_id: &Option<String>,
    mut recv: quinn::RecvStream,
) {
    match timeout(ctx.inbound_read_timeout, read_framed(&mut recv)).await {
        Ok(Ok(bytes)) => match serde_json::from_slice::<Envelope>(&bytes) {
            Ok(envelope) => {
                if authenticated_peer_id.as_deref() != Some(envelope.from.as_str()) {
                    debug!("dropping uni message before hello authentication");
                } else if envelope.kind == MessageKind::Unknown {
                    debug!("dropping unknown kind on uni stream");
                } else if envelope.kind.expects_response() {
                    debug!("dropping request kind on uni stream");
                } else if let Err(err) = envelope.validate() {
                    debug!(error = %err, "dropping invalid uni envelope");
                } else {
                    if let Some(ref check) = ctx.replay_check
                        && check(envelope.id).await
                    {
                        debug!(msg_id = %envelope.id, "dropping replayed uni envelope");
                        return;
                    }
                    let _ = ctx.inbound_tx.send(Arc::new(envelope));
                }
            }
            Err(err) => {
                debug!(error = %err, "dropping malformed uni envelope");
            }
        },
        Ok(Err(err)) => {
            warn!(error = %err, peer = ?authenticated_peer_id, "failed reading uni stream");
        }
        Err(_) => {
            warn!(peer = ?authenticated_peer_id, "uni stream read timed out");
        }
    }
}

// ---------------------------------------------------------------------------
// Bidirectional stream handler
// ---------------------------------------------------------------------------

/// Handle an authenticated bidi request (post-hello).
async fn handle_authenticated_bidi(
    ctx: &ConnectionContext,
    request: Envelope,
    mut send: quinn::SendStream,
) {
    if request.kind == MessageKind::Unknown {
        let response = Envelope::response_to(
            &request,
            ctx.local_agent_id.clone(),
            MessageKind::Error,
            serde_json::to_value(ErrorPayload {
                code: ErrorCode::UnknownKind,
                message: "unknown message kind on bidirectional stream".to_string(),
                retryable: false,
            })
            .unwrap(),
        );
        send_response(&mut send, &response).await;
    } else if !request.kind.expects_response() {
        // Fire-and-forget kind on a bidi stream — accept it gracefully
        if let Err(err) = request.validate() {
            debug!(error = %err, "dropping invalid bidi fire-and-forget envelope");
        } else {
            if let Some(ref check) = ctx.replay_check
                && check(request.id).await
            {
                debug!(msg_id = %request.id, "dropping replayed bidi fire-and-forget envelope");
                let _ = send.finish();
                return;
            }
            let _ = ctx.inbound_tx.send(Arc::new(request));
        }
        let _ = send.finish();
    } else if let Err(err) = request.validate() {
        let response = Envelope::response_to(
            &request,
            ctx.local_agent_id.clone(),
            MessageKind::Error,
            serde_json::to_value(ErrorPayload {
                code: ErrorCode::InvalidEnvelope,
                message: format!("envelope validation failed: {err}"),
                retryable: false,
            })
            .unwrap(),
        );
        send_response(&mut send, &response).await;
    } else {
        if let Some(ref check) = ctx.replay_check
            && check(request.id).await
        {
            debug!(msg_id = %request.id, "dropping replayed bidi request");
            return;
        }
        let request_arc = Arc::new(request.clone());
        let _ = ctx.inbound_tx.send(request_arc.clone());
        let response = if let Some(ref handler) = ctx.response_handler {
            match handler(request_arc).await {
                Some(resp) => resp,
                None => auto_response(&request, &ctx.local_agent_id),
            }
        } else {
            auto_response(&request, &ctx.local_agent_id)
        };
        send_response(&mut send, &response).await;
    }
}

/// Handle the hello handshake on a bidi stream. Returns the authenticated peer
/// ID on success, or None if the handshake failed (connection should close).
async fn handle_hello(
    ctx: &ConnectionContext,
    request: Envelope,
    mut send: quinn::SendStream,
) -> Option<String> {
    let hello_resp =
        match validate_hello_identity(&request, &ctx.peer_cert_pubkey_b64, &ctx.expected_pubkeys) {
            Ok(()) => {
                if let Err(err) = request.validate() {
                    Envelope::response_to(
                        &request,
                        ctx.local_agent_id.clone(),
                        MessageKind::Error,
                        serde_json::to_value(ErrorPayload {
                            code: ErrorCode::InvalidEnvelope,
                            message: format!("hello envelope validation failed: {err}"),
                            retryable: false,
                        })
                        .unwrap(),
                    )
                } else {
                    auto_response(&request, &ctx.local_agent_id)
                }
            }
            Err(err) => Envelope::response_to(
                &request,
                ctx.local_agent_id.clone(),
                MessageKind::Error,
                serde_json::to_value(ErrorPayload {
                    code: ErrorCode::NotAuthorized,
                    message: err,
                    retryable: false,
                })
                .unwrap(),
            ),
        };

    let is_success = hello_resp.kind == MessageKind::Hello;
    send_response(&mut send, &hello_resp).await;

    if is_success {
        let peer_id = request.from.to_string();
        ctx.connections
            .write()
            .await
            .insert(peer_id.clone(), ctx.connection.clone());
        let _ = ctx.inbound_tx.send(Arc::new(request));
        Some(peer_id)
    } else {
        None
    }
}

async fn handle_bidi_stream(
    ctx: &ConnectionContext,
    authenticated_peer_id: &mut Option<String>,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
) -> bool {
    let request = match timeout(ctx.inbound_read_timeout, read_framed(&mut recv)).await {
        Ok(Ok(bytes)) => match serde_json::from_slice::<Envelope>(&bytes) {
            Ok(r) => r,
            Err(err) => {
                debug!(error = %err, "dropping malformed bidi envelope");
                return true;
            }
        },
        Ok(Err(err)) => {
            warn!(error = %err, peer = ?authenticated_peer_id, "failed reading bidi stream");
            return true;
        }
        Err(_) => {
            warn!(peer = ?authenticated_peer_id, "bidi stream read timed out");
            return true;
        }
    };

    if request.kind == MessageKind::Hello {
        match handle_hello(ctx, request, send).await {
            Some(peer_id) => {
                *authenticated_peer_id = Some(peer_id);
                return true;
            }
            None => return false, // hello failed → close connection
        }
    }

    if authenticated_peer_id.as_deref() != Some(request.from.as_str()) {
        let response = Envelope::response_to(
            &request,
            ctx.local_agent_id.clone(),
            MessageKind::Error,
            serde_json::to_value(ErrorPayload {
                code: ErrorCode::NotAuthorized,
                message: "hello handshake must complete before other requests".to_string(),
                retryable: false,
            })
            .unwrap(),
        );
        send_response(&mut send, &response).await;
        return true;
    }

    handle_authenticated_bidi(ctx, request, send).await;
    true
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_response(send: &mut quinn::SendStream, response: &Envelope) {
    if let Ok(response_bytes) = serde_json::to_vec(response)
        && write_framed(send, &response_bytes).await.is_ok()
    {
        let _ = send.finish();
    }
}

// ---------------------------------------------------------------------------
// Connection loop
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_connection(
    connection: quinn::Connection,
    local_agent_id: String,
    mut authenticated_peer_id: Option<String>,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
    connections: Arc<RwLock<HashMap<String, quinn::Connection>>>,
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
    cancel: CancellationToken,
    replay_check: Option<ReplayCheckFn>,
    response_handler: Option<ResponseHandlerFn>,
    handshake_timeout: Duration,
    inbound_read_timeout: Duration,
) {
    let peer_cert_pubkey_b64 = match extract_peer_pubkey_base64_from_connection(&connection) {
        Ok(pubkey) => pubkey,
        Err(err) => {
            warn!(error = %err, "failed to extract peer cert public key");
            return;
        }
    };

    let ctx = ConnectionContext {
        connection: connection.clone(),
        local_agent_id,
        peer_cert_pubkey_b64,
        inbound_tx,
        connections,
        expected_pubkeys,
        replay_check,
        response_handler,
        inbound_read_timeout,
    };

    let handshake_deadline = tokio::time::Instant::now() + handshake_timeout;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("connection loop shutting down via cancellation");
                break;
            }
            _ = tokio::time::sleep_until(handshake_deadline), if authenticated_peer_id.is_none() => {
                warn!("closing connection: hello handshake not completed within {:?}", handshake_timeout);
                connection.close(0u32.into(), b"handshake timeout");
                break;
            }
            uni = connection.accept_uni() => {
                match uni {
                    Ok(recv) => handle_uni_stream(&ctx, &authenticated_peer_id, recv).await,
                    Err(_) => break,
                }
            }
            bi = connection.accept_bi() => {
                match bi {
                    Ok((send, recv)) => {
                        if !handle_bidi_stream(&ctx, &mut authenticated_peer_id, send, recv).await {
                            break; // hello failed
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    if let Some(peer_id) = authenticated_peer_id {
        ctx.connections.write().await.remove(&peer_id);
    }
}
