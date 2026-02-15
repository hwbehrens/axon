use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
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
        .downcast::<Vec<rustls::Certificate>>()
        .map_err(|_| anyhow!("peer identity was not a rustls certificate chain"))?;

    let cert = certs
        .first()
        .ok_or_else(|| anyhow!("peer certificate chain is empty"))?;

    let key = extract_ed25519_pubkey_from_cert_der(&cert.0)?;
    Ok(STANDARD.encode(key))
}

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
                    Ok(mut recv) => {
                        match timeout(inbound_read_timeout, read_framed(&mut recv)).await {
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
                                        if let Some(ref check) = replay_check
                                            && check(envelope.id).await
                                        {
                                            debug!(msg_id = %envelope.id, "dropping replayed uni envelope");
                                            continue;
                                        }
                                        let _ = inbound_tx.send(Arc::new(envelope));
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
                    Err(_) => break,
                }
            }
            bi = connection.accept_bi() => {
                match bi {
                    Ok((mut send, mut recv)) => {
                        let request = match timeout(inbound_read_timeout, read_framed(&mut recv)).await {
                            Ok(Ok(bytes)) => match serde_json::from_slice::<Envelope>(&bytes) {
                                Ok(r) => r,
                                Err(err) => {
                                    debug!(error = %err, "dropping malformed bidi envelope");
                                    continue;
                                }
                            },
                            Ok(Err(err)) => {
                                warn!(error = %err, peer = ?authenticated_peer_id, "failed reading bidi stream");
                                continue;
                            }
                            Err(_) => {
                                warn!(peer = ?authenticated_peer_id, "bidi stream read timed out");
                                continue;
                            }
                        };

                        if request.kind == MessageKind::Hello {
                            let hello_resp = match validate_hello_identity(&request, &peer_cert_pubkey_b64, &expected_pubkeys) {
                                Ok(()) => {
                                    if let Err(err) = request.validate() {
                                        Envelope::response_to(
                                            &request,
                                            local_agent_id.clone(),
                                            MessageKind::Error,
                                            serde_json::to_value(ErrorPayload {
                                                code: ErrorCode::InvalidEnvelope,
                                                message: format!("hello envelope validation failed: {err}"),
                                                retryable: false,
                                            })
                                            .unwrap(),
                                        )
                                    } else {
                                        auto_response(&request, &local_agent_id)
                                    }
                                }
                                Err(err) => Envelope::response_to(
                                    &request,
                                    local_agent_id.clone(),
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
                            if let Ok(response_bytes) = serde_json::to_vec(&hello_resp)
                                && write_framed(&mut send, &response_bytes).await.is_ok()
                            {
                                let _ = send.finish().await;
                            }
                            if is_success {
                                authenticated_peer_id = Some(request.from.to_string());
                                connections
                                    .write()
                                    .await
                                    .insert(request.from.to_string(), connection.clone());
                                let _ = inbound_tx.send(Arc::new(request));
                            } else {
                                break;
                            }
                        } else if authenticated_peer_id.as_deref() != Some(request.from.as_str()) {
                            let response = Envelope::response_to(
                                &request,
                                local_agent_id.clone(),
                                MessageKind::Error,
                                serde_json::to_value(ErrorPayload {
                                    code: ErrorCode::NotAuthorized,
                                    message: "hello handshake must complete before other requests"
                                        .to_string(),
                                    retryable: false,
                                })
                                .unwrap(),
                            );
                            if let Ok(response_bytes) = serde_json::to_vec(&response)
                                && write_framed(&mut send, &response_bytes).await.is_ok()
                            {
                                let _ = send.finish().await;
                            }
                        } else if request.kind == MessageKind::Unknown {
                            let response = Envelope::response_to(
                                &request,
                                local_agent_id.clone(),
                                MessageKind::Error,
                                serde_json::to_value(ErrorPayload {
                                    code: ErrorCode::UnknownKind,
                                    message: "unknown message kind on bidirectional stream"
                                        .to_string(),
                                    retryable: false,
                                })
                                .unwrap(),
                            );
                            if let Ok(response_bytes) = serde_json::to_vec(&response)
                                && write_framed(&mut send, &response_bytes).await.is_ok()
                            {
                                let _ = send.finish().await;
                            }
                        } else if !request.kind.expects_response() {
                            if let Err(err) = request.validate() {
                                debug!(error = %err, "dropping invalid bidi fire-and-forget envelope");
                            } else {
                                if let Some(ref check) = replay_check
                                    && check(request.id).await
                                {
                                    debug!(msg_id = %request.id, "dropping replayed bidi fire-and-forget envelope");
                                    let _ = send.finish().await;
                                    continue;
                                }
                                let _ = inbound_tx.send(Arc::new(request));
                            }
                            let _ = send.finish().await;
                        } else if let Err(err) = request.validate() {
                            let response = Envelope::response_to(
                                &request,
                                local_agent_id.clone(),
                                MessageKind::Error,
                                serde_json::to_value(ErrorPayload {
                                    code: ErrorCode::InvalidEnvelope,
                                    message: format!("envelope validation failed: {err}"),
                                    retryable: false,
                                })
                                .unwrap(),
                            );
                            if let Ok(response_bytes) = serde_json::to_vec(&response)
                                && write_framed(&mut send, &response_bytes).await.is_ok()
                            {
                                let _ = send.finish().await;
                            }
                        } else {
                            if let Some(ref check) = replay_check
                                && check(request.id).await
                            {
                                debug!(msg_id = %request.id, "dropping replayed bidi request");
                                continue;
                            }
                            let request_arc = Arc::new(request.clone());
                            let _ = inbound_tx.send(request_arc.clone());
                            let response = if let Some(ref handler) = response_handler {
                                match handler(request_arc).await {
                                    Some(resp) => resp,
                                    None => auto_response(&request, &local_agent_id),
                                }
                            } else {
                                auto_response(&request, &local_agent_id)
                            };
                            if let Ok(response_bytes) = serde_json::to_vec(&response)
                                && write_framed(&mut send, &response_bytes).await.is_ok()
                            {
                                let _ = send.finish().await;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    if let Some(peer_id) = authenticated_peer_id {
        connections.write().await.remove(&peer_id);
    }
}
