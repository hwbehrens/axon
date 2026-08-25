use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustls::pki_types::CertificateDer;
use serde_json::json;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::message::{AgentId, Envelope, MessageKind};

use super::MAX_MESSAGE_SIZE_USIZE;
use super::quic_transport::ResponseHandlerFn;
use super::tls::{derive_agent_id_from_pubkey_bytes, extract_ed25519_pubkey_from_cert_der};

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

/// A transport send failure annotated with delivery ambiguity.
///
/// `ambiguous` is true once payload bytes may already have reached the peer:
/// retrying such a send can duplicate application delivery, which violates
/// AXON's documented at-most-once guarantee for fire-and-forget messages.
/// Failures that occur before any payload byte is written are provably
/// undelivered and safe to refresh-and-retry.
///
/// `timed_out` marks budget exhaustion so callers can surface the distinct
/// `timeout` contract instead of `peer_unreachable` (spec/IPC.md §5).
#[derive(Debug)]
pub struct SendError {
    /// Underlying failure.
    pub inner: anyhow::Error,
    /// True once payload bytes may already have reached the peer.
    pub ambiguous: bool,
    /// True when the failure was budget exhaustion rather than an error.
    pub timed_out: bool,
}

impl SendError {
    pub(crate) fn pre_send(inner: anyhow::Error) -> Self {
        Self {
            inner,
            ambiguous: false,
            timed_out: false,
        }
    }

    fn ambiguous(inner: anyhow::Error) -> Self {
        Self {
            inner,
            ambiguous: true,
            timed_out: false,
        }
    }

    /// Budget exhaustion before delivery (dial/handshake/write deadlines).
    pub(crate) fn pre_send_timeout(inner: anyhow::Error) -> Self {
        Self {
            inner,
            ambiguous: false,
            timed_out: true,
        }
    }

    /// Budget exhaustion after bytes may have been written.
    fn timeout(inner: anyhow::Error) -> Self {
        Self {
            inner,
            ambiguous: true,
            timed_out: true,
        }
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

/// Whether a failed exchange may be retried on a refreshed connection.
/// Requests keep the single documented transport-level retry (DEC-016):
/// their reply correlation is specified as at-most-one-reply, not
/// exactly-once execution. Fire-and-forget kinds may only be retried when
/// the failure is provably pre-delivery.
pub(crate) fn retry_permitted(kind: &MessageKind, error: &SendError) -> bool {
    kind.expects_response() || !error.ambiguous
}

pub(crate) async fn send_unidirectional(
    connection: &quinn::Connection,
    envelope: Envelope,
    budget: Duration,
) -> Result<(), SendError> {
    let bytes = envelope
        .wire_encode()
        .map_err(|err| SendError::pre_send(err.context("failed to serialize envelope for wire")))?;

    let mut stream = connection.open_uni().await.map_err(|err| {
        SendError::pre_send(anyhow::Error::new(err).context("failed to open uni stream"))
    })?;
    // Past this point the payload may reach the peer: every failure is
    // classified ambiguous.
    tokio::time::timeout(budget, write_framed(&mut stream, &bytes))
        .await
        .map_err(|_| SendError::timeout(anyhow!("uni frame write exceeded send budget")))?
        .map_err(|err| SendError::ambiguous(err.context("uni frame write failed")))?;
    stream.finish().map_err(|err| {
        SendError::ambiguous(anyhow::Error::new(err).context("failed to finish uni stream"))
    })?;
    Ok(())
}

pub(crate) async fn send_request(
    connection: &quinn::Connection,
    envelope: Envelope,
    local_agent_id: &AgentId,
    request_timeout: Duration,
) -> Result<Envelope, SendError> {
    let bytes = envelope
        .wire_encode()
        .map_err(|err| SendError::pre_send(err.context("failed to serialize request for wire")))?;

    let (mut send, mut recv) = connection.open_bi().await.map_err(|err| {
        SendError::pre_send(anyhow::Error::new(err).context("failed to open bidi stream"))
    })?;
    write_framed(&mut send, &bytes)
        .await
        .map_err(|err| SendError::ambiguous(err.context("request frame write failed")))?;
    send.finish().map_err(|err| {
        SendError::ambiguous(anyhow::Error::new(err).context("failed to finish request stream"))
    })?;

    let timeout_label = if request_timeout.as_millis() < 1000 {
        format!("{}ms", request_timeout.as_millis())
    } else {
        format!("{}s", request_timeout.as_secs())
    };
    let response_bytes = timeout(request_timeout, read_framed(&mut recv))
        .await
        .map_err(|_| SendError::timeout(anyhow!("request timed out after {timeout_label}")))?
        .map_err(SendError::ambiguous)?;
    let mut response = serde_json::from_slice::<Envelope>(&response_bytes).map_err(|err| {
        SendError::ambiguous(anyhow::Error::new(err).context("failed to decode response envelope"))
    })?;
    response
        .validate()
        .map_err(|err| SendError::ambiguous(err.context("response envelope failed validation")))?;
    validate_bidi_response(&response, envelope.id).map_err(SendError::ambiguous)?;
    let peer_id = derive_peer_id_from_connection(connection).map_err(SendError::ambiguous)?;
    overwrite_authenticated_identity(&mut response, &peer_id, local_agent_id);
    Ok(response)
}

fn validate_bidi_response(response: &Envelope, request_id: Uuid) -> Result<()> {
    if !response.kind.is_response() {
        bail!(
            "bidirectional reply must use response|error kind, got {}",
            response.kind
        );
    }
    if response.ref_id != Some(request_id) {
        bail!(
            "bidirectional reply ref {:?} does not match request {}",
            response.ref_id,
            request_id
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Default error response for unhandled bidi requests
// ---------------------------------------------------------------------------

/// Default response for unhandled bidi requests when no response handler is
/// registered (or the handler returns `None`).
pub fn default_error_response(request: &Envelope, local_agent_id: &str) -> Envelope {
    Envelope::response_to(
        request,
        AgentId::parse(local_agent_id).expect("transport local Agent ID is validated at bind"),
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

pub(crate) fn derive_peer_id_from_connection(connection: &quinn::Connection) -> Result<String> {
    let peer_cert_pubkey_b64 = extract_peer_pubkey_base64_from_connection(connection)?;
    let pubkey_bytes = STANDARD
        .decode(&peer_cert_pubkey_b64)
        .context("failed to decode peer cert public key from base64")?;
    Ok(derive_agent_id_from_pubkey_bytes(&pubkey_bytes))
}

fn overwrite_authenticated_identity(
    envelope: &mut Envelope,
    peer_id: &str,
    local_agent_id: &AgentId,
) {
    envelope.from = Some(
        AgentId::parse(peer_id).expect("peer Agent ID is derived from authenticated key material"),
    );
    envelope.to = Some(local_agent_id.clone());
}

// ---------------------------------------------------------------------------
// Connection context — shared state for stream handlers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ConnectionContext {
    local_agent_id: AgentId,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
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
async fn handle_authenticated_bidi(
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_connection(
    connection: quinn::Connection,
    local_agent_id: AgentId,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
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

    let ctx = Arc::new(ConnectionContext {
        local_agent_id,
        inbound_tx,
        response_handler,
        inbound_read_timeout,
    });
    let mut streams = JoinSet::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("connection loop shutting down via cancellation");
                break;
            }
            uni = connection.accept_uni() => {
                match uni {
                    Ok(recv) => {
                        let ctx = ctx.clone();
                        let peer_id = peer_id.clone();
                        streams.spawn(async move {
                            handle_uni_stream(&ctx, &peer_id, recv).await;
                        });
                    }
                    Err(_) => break,
                }
            }
            bi = connection.accept_bi() => {
                match bi {
                    Ok((send, recv)) => {
                        let ctx = ctx.clone();
                        let peer_id = peer_id.clone();
                        streams.spawn(async move {
                            handle_bidi_stream(&ctx, &peer_id, send, recv).await;
                        });
                    }
                    Err(_) => break,
                }
            }
            finished = streams.join_next(), if !streams.is_empty() => {
                // Reap completed stream tasks so handles cannot accumulate
                // over a long-lived connection's lifetime.
                match finished {
                    Some(Ok(())) => {}
                    Some(Err(err)) => warn!(error = %err, "stream task failed"),
                    None => {}
                }
            }
        }
    }

    streams.abort_all();
    while streams.join_next().await.is_some() {}
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
