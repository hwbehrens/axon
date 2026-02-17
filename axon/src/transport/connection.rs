use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustls::pki_types::CertificateDer;
use serde_json::json;
use tokio::sync::{RwLock, broadcast};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::message::{Envelope, MessageKind};

use super::quic_transport::ResponseHandlerFn;
use super::tls::{derive_agent_id_from_pubkey_bytes, extract_ed25519_pubkey_from_cert_der};
use super::{MAX_MESSAGE_SIZE_USIZE, REQUEST_TIMEOUT};

// ---------------------------------------------------------------------------
// Framing helpers — length-delimited read/write on QUIC streams
// ---------------------------------------------------------------------------

pub(crate) async fn write_framed(stream: &mut quinn::SendStream, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!("message too large for framing"));
    }

    stream
        .write_all(bytes)
        .await
        .context("failed to write frame body")?;
    Ok(())
}

pub(crate) async fn read_framed(stream: &mut quinn::RecvStream) -> Result<Vec<u8>> {
    let buf = stream
        .read_to_end(MAX_MESSAGE_SIZE_USIZE)
        .await
        .context("failed to read frame body")?;
    Ok(buf)
}

pub(crate) async fn send_unidirectional(
    connection: &quinn::Connection,
    envelope: Envelope,
) -> Result<()> {
    let bytes = envelope
        .wire_encode()
        .context("failed to serialize envelope for wire")?;

    let mut stream = connection
        .open_uni()
        .await
        .context("failed to open uni stream")?;
    write_framed(&mut stream, &bytes).await?;
    stream.finish().context("failed to finish uni stream")?;
    Ok(())
}

pub(crate) async fn send_request(
    connection: &quinn::Connection,
    envelope: Envelope,
    local_agent_id: &str,
) -> Result<Envelope> {
    let bytes = envelope
        .wire_encode()
        .context("failed to serialize request for wire")?;

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("failed to open bidi stream")?;
    write_framed(&mut send, &bytes).await?;
    send.finish().context("failed to finish request stream")?;

    let response_bytes = timeout(REQUEST_TIMEOUT, read_framed(&mut recv))
        .await
        .context("request timed out after 30s")??;
    let mut response = serde_json::from_slice::<Envelope>(&response_bytes)
        .context("failed to decode response envelope")?;
    response
        .validate()
        .context("response envelope failed validation")?;
    let peer_id = derive_peer_id_from_connection(connection)?;
    overwrite_authenticated_identity(&mut response, &peer_id, local_agent_id);
    Ok(response)
}

// ---------------------------------------------------------------------------
// Default error response for unhandled bidi requests
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Peer public-key extraction
// ---------------------------------------------------------------------------

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

fn derive_peer_id_from_connection(connection: &quinn::Connection) -> Result<String> {
    let peer_cert_pubkey_b64 = extract_peer_pubkey_base64_from_connection(connection)?;
    let pubkey_bytes = STANDARD
        .decode(&peer_cert_pubkey_b64)
        .context("failed to decode peer cert public key from base64")?;
    Ok(derive_agent_id_from_pubkey_bytes(&pubkey_bytes))
}

fn overwrite_authenticated_identity(envelope: &mut Envelope, peer_id: &str, local_agent_id: &str) {
    envelope.from = Some(peer_id.into());
    envelope.to = Some(local_agent_id.into());
}

// ---------------------------------------------------------------------------
// Connection context — shared state for stream handlers
// ---------------------------------------------------------------------------

struct ConnectionContext {
    connection: quinn::Connection,
    local_agent_id: String,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
    connections: Arc<RwLock<HashMap<String, quinn::Connection>>>,
    response_handler: Option<ResponseHandlerFn>,
    inbound_read_timeout: Duration,
}

// ---------------------------------------------------------------------------
// Unidirectional stream handler
// ---------------------------------------------------------------------------

async fn handle_uni_stream(ctx: &ConnectionContext, peer_id: &str, mut recv: quinn::RecvStream) {
    match timeout(ctx.inbound_read_timeout, read_framed(&mut recv)).await {
        Ok(Ok(bytes)) => match serde_json::from_slice::<Envelope>(&bytes) {
            Ok(mut envelope) => {
                overwrite_authenticated_identity(&mut envelope, peer_id, &ctx.local_agent_id);
                if envelope.kind == MessageKind::Unknown {
                    debug!("dropping unknown kind on uni stream");
                } else if envelope.kind.expects_response() {
                    debug!("dropping request kind on uni stream");
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
            json!({
                "code": "unknown_kind",
                "message": "unknown message kind on bidirectional stream",
                "retryable": false,
            }),
        );
        send_response(&mut send, &response).await;
    } else if !request.kind.expects_response() {
        // Fire-and-forget kind on a bidi stream — accept it gracefully
        if let Err(err) = request.validate() {
            debug!(error = %err, "dropping invalid bidi fire-and-forget envelope");
        } else {
            let _ = ctx.inbound_tx.send(Arc::new(request));
        }
        let _ = send.finish();
    } else if let Err(err) = request.validate() {
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
    } else {
        let request_arc = Arc::new(request.clone());
        let _ = ctx.inbound_tx.send(request_arc.clone());
        let response = if let Some(ref handler) = ctx.response_handler {
            match handler(request_arc).await {
                Some(resp) => resp,
                None => default_error_response(&request, &ctx.local_agent_id),
            }
        } else {
            default_error_response(&request, &ctx.local_agent_id)
        };
        send_response(&mut send, &response).await;
    }
}

async fn handle_bidi_stream(
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

// ---------------------------------------------------------------------------
// Connection loop
// ---------------------------------------------------------------------------

pub(crate) async fn run_connection(
    connection: quinn::Connection,
    local_agent_id: String,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
    connections: Arc<RwLock<HashMap<String, quinn::Connection>>>,
    cancel: CancellationToken,
    response_handler: Option<ResponseHandlerFn>,
    inbound_read_timeout: Duration,
) {
    let peer_id = match derive_peer_id_from_connection(&connection) {
        Ok(peer_id) => peer_id,
        Err(err) => {
            warn!(error = %err, "failed to derive peer id from TLS identity");
            return;
        }
    };

    let ctx = ConnectionContext {
        connection: connection.clone(),
        local_agent_id,
        inbound_tx,
        connections,
        response_handler,
        inbound_read_timeout,
    };

    let my_stable_id = ctx.connection.stable_id();
    ctx.connections
        .write()
        .await
        .insert(peer_id.clone(), ctx.connection.clone());

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("connection loop shutting down via cancellation");
                break;
            }
            uni = connection.accept_uni() => {
                match uni {
                    Ok(recv) => handle_uni_stream(&ctx, &peer_id, recv).await,
                    Err(_) => break,
                }
            }
            bi = connection.accept_bi() => {
                match bi {
                    Ok((send, recv)) => {
                        handle_bidi_stream(&ctx, &peer_id, send, recv).await;
                    }
                    Err(_) => break,
                }
            }
        }
    }

    // Only remove our entry if we're still the registered connection.
    // Another connection loop (from a simultaneous dial) may have replaced us.
    let mut conns = ctx.connections.write().await;
    if conns
        .get(&peer_id)
        .is_some_and(|c| c.stable_id() == my_stable_id)
    {
        conns.remove(&peer_id);
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
