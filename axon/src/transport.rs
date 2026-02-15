use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustls::DistinguishedName;
use rustls::client::{ServerCertVerified, ServerCertVerifier};
use rustls::server::{ClientCertVerified, ClientCertVerifier};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use x509_parser::prelude::*;

use crate::identity::{Identity, QuicCertificate};
use crate::message::{
    Envelope, HelloPayload, MAX_MESSAGE_SIZE, MessageKind, hello_features, now_millis,
};
use crate::peer_table::PeerRecord;

pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_MESSAGE_SIZE_USIZE: usize = MAX_MESSAGE_SIZE as usize;

// ---------------------------------------------------------------------------
// QuicTransport
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct QuicTransport {
    endpoint: quinn::Endpoint,
    local_agent_id: String,
    connections: Arc<RwLock<HashMap<String, quinn::Connection>>>,
    /// Per-peer lock to prevent concurrent connection attempts to the same peer.
    connecting_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    // std::sync required: shared with TLS verifier callbacks which are synchronous
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
    inbound_tx: broadcast::Sender<Envelope>,
    cancel: CancellationToken,
}

impl QuicTransport {
    pub async fn bind(bind_addr: SocketAddr, identity: &Identity) -> Result<Self> {
        Self::bind_cancellable(bind_addr, identity, CancellationToken::new()).await
    }

    pub async fn bind_cancellable(
        bind_addr: SocketAddr,
        identity: &Identity,
        cancel: CancellationToken,
    ) -> Result<Self> {
        let cert = identity.make_quic_certificate()?;
        let expected_pubkeys = Arc::new(StdRwLock::new(HashMap::new()));
        let (endpoint, inbound_tx) = build_endpoint(bind_addr, &cert, expected_pubkeys.clone())?;

        let transport = Self {
            endpoint,
            local_agent_id: identity.agent_id().to_string(),
            connections: Arc::new(RwLock::new(HashMap::new())),
            connecting_locks: Arc::new(RwLock::new(HashMap::new())),
            expected_pubkeys,
            inbound_tx,
            cancel,
        };
        transport.spawn_accept_loop();
        Ok(transport)
    }

    pub fn subscribe_inbound(&self) -> broadcast::Receiver<Envelope> {
        self.inbound_tx.subscribe()
    }

    pub fn set_expected_peer(&self, agent_id: String, pubkey: String) {
        if let Ok(mut map) = self.expected_pubkeys.write() {
            map.insert(agent_id, pubkey);
        }
    }

    pub async fn has_connection(&self, agent_id: &str) -> bool {
        self.connections.read().await.contains_key(agent_id)
    }

    pub async fn ensure_connection(&self, peer: &PeerRecord) -> Result<quinn::Connection> {
        self.set_expected_peer(peer.agent_id.clone(), peer.pubkey.clone());

        // Fast path: already connected.
        if let Some(existing) = self.connections.read().await.get(&peer.agent_id).cloned() {
            return Ok(existing);
        }

        // Acquire per-peer lock to prevent duplicate concurrent connection attempts.
        let peer_lock = {
            let mut locks = self.connecting_locks.write().await;
            locks
                .entry(peer.agent_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = peer_lock.lock().await;

        // Re-check after acquiring the lock — another task may have connected.
        if let Some(existing) = self.connections.read().await.get(&peer.agent_id).cloned() {
            return Ok(existing);
        }

        let connecting = self
            .endpoint
            .connect(peer.addr, &peer.agent_id)
            .with_context(|| format!("failed to begin QUIC connect to {}", peer.addr))?;

        let connection = connecting
            .await
            .with_context(|| format!("QUIC handshake failed with {}", peer.addr))?;

        self.perform_hello(&connection, &peer.agent_id).await?;
        self.spawn_connection_loop(connection.clone(), Some(peer.agent_id.clone()));
        self.connections
            .write()
            .await
            .insert(peer.agent_id.clone(), connection.clone());

        Ok(connection)
    }

    pub async fn send(&self, peer: &PeerRecord, envelope: Envelope) -> Result<Option<Envelope>> {
        let connection = self.ensure_connection(peer).await?;

        if envelope.kind.expects_response() {
            let response = send_request(&connection, envelope).await?;
            Ok(Some(response))
        } else {
            send_unidirectional(&connection, envelope).await?;
            Ok(None)
        }
    }

    pub async fn close_all(&self) {
        for connection in self.connections.read().await.values() {
            connection.close(0u32.into(), b"shutdown");
        }
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint
            .local_addr()
            .context("failed to get local address")
    }

    async fn perform_hello(
        &self,
        connection: &quinn::Connection,
        remote_agent_id: &str,
    ) -> Result<()> {
        let payload = serde_json::to_value(HelloPayload {
            protocol_versions: vec![1],
            selected_version: None,
            agent_name: None,
            features: hello_features(),
        })
        .context("failed to encode hello payload")?;

        let hello = Envelope {
            v: 1,
            id: uuid::Uuid::new_v4(),
            from: self.local_agent_id.clone(),
            to: remote_agent_id.to_string(),
            ts: now_millis(),
            kind: MessageKind::Hello,
            ref_id: None,
            payload,
        };

        let response = send_request(connection, hello).await?;
        match response.kind {
            MessageKind::Hello => {
                let selected = response
                    .payload
                    .get("selected_version")
                    .and_then(|v| v.as_u64());
                if selected != Some(1) {
                    return Err(anyhow!(
                        "peer {} did not negotiate protocol version 1. \
                         Local agent supports: [1]. Peer selected: {:?}. \
                         Check that the peer is running a compatible AXON version.",
                        remote_agent_id,
                        selected,
                    ));
                }
            }
            MessageKind::Error => {
                let code = response
                    .payload
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown_error");
                let message = response
                    .payload
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("peer rejected hello");
                return Err(anyhow!(
                    "peer {remote_agent_id} rejected hello: {code}: {message}. \
                     Check that the peer accepts connections from this agent."
                ));
            }
            other => {
                return Err(anyhow!(
                    "unexpected response kind '{other}' during hello handshake with {remote_agent_id}. \
                     Expected 'hello' or 'error'."
                ));
            }
        }
        Ok(())
    }

    fn spawn_accept_loop(&self) {
        let endpoint = self.endpoint.clone();
        let inbound_tx = self.inbound_tx.clone();
        let local_id = self.local_agent_id.clone();
        let connections = self.connections.clone();
        let expected_pubkeys = self.expected_pubkeys.clone();
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("accept loop shutting down");
                        break;
                    }
                    maybe_conn = endpoint.accept() => {
                        let Some(connecting) = maybe_conn else { break };
                        match connecting.await {
                            Ok(connection) => {
                                debug!(remote = ?connection.remote_address(), "accepted inbound QUIC connection");
                                let inbound_tx = inbound_tx.clone();
                                let local_id = local_id.clone();
                                let connections = connections.clone();
                                let expected_pubkeys = expected_pubkeys.clone();
                                let cancel = cancel.clone();
                                tokio::spawn(async move {
                                    run_connection(
                                        connection,
                                        local_id,
                                        None,
                                        inbound_tx,
                                        connections,
                                        expected_pubkeys,
                                        cancel,
                                    )
                                    .await;
                                });
                            }
                            Err(err) => warn!(error = %err, "failed to accept QUIC connection"),
                        }
                    }
                }
            }
        });
    }

    fn spawn_connection_loop(
        &self,
        connection: quinn::Connection,
        initial_authenticated_peer_id: Option<String>,
    ) {
        let inbound_tx = self.inbound_tx.clone();
        let local_id = self.local_agent_id.clone();
        let connections = self.connections.clone();
        let expected_pubkeys = self.expected_pubkeys.clone();
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            run_connection(
                connection,
                local_id,
                initial_authenticated_peer_id,
                inbound_tx,
                connections,
                expected_pubkeys,
                cancel,
            )
            .await;
        });
    }
}

// ---------------------------------------------------------------------------
// Endpoint construction
// ---------------------------------------------------------------------------

fn build_endpoint(
    bind_addr: SocketAddr,
    cert: &QuicCertificate,
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
) -> Result<(quinn::Endpoint, broadcast::Sender<Envelope>)> {
    let cert_chain = vec![rustls::Certificate(cert.cert_der.clone())];
    let private_key = rustls::PrivateKey(cert.key_der.clone());

    let subject_dn = extract_subject_dn_from_cert_der(&cert.cert_der)
        .context("failed to extract certificate subject for mTLS")?;
    let mtls_verifier = PeerClientCertVerifier {
        expected_pubkeys: expected_pubkeys.clone(),
        roots: vec![DistinguishedName::from(subject_dn)],
    };

    let mut rustls_server = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_client_cert_verifier(Arc::new(mtls_verifier))
        .with_single_cert(cert_chain.clone(), private_key.clone())
        .context("failed to build rustls server config")?;
    rustls_server.max_early_data_size = u32::MAX;

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(rustls_server));

    let transport_config = Arc::new({
        let mut config = quinn::TransportConfig::default();
        config.keep_alive_interval(Some(KEEPALIVE_INTERVAL));
        if let Ok(idle) = quinn::IdleTimeout::try_from(IDLE_TIMEOUT) {
            config.max_idle_timeout(Some(idle));
        }
        config
    });
    server_config.transport = transport_config.clone();

    let mut endpoint = quinn::Endpoint::server(server_config, bind_addr)
        .with_context(|| format!("failed to bind QUIC endpoint on {bind_addr}"))?;

    let mut rustls_client = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(PeerCertVerifier { expected_pubkeys }))
        .with_client_auth_cert(cert_chain, private_key)
        .context("failed to configure client mTLS certificate")?;
    rustls_client.enable_early_data = true;

    let mut client_config = quinn::ClientConfig::new(Arc::new(rustls_client));
    client_config.transport_config(transport_config);
    endpoint.set_default_client_config(client_config);

    let (inbound_tx, _) = broadcast::channel(512);
    Ok((endpoint, inbound_tx))
}

// ---------------------------------------------------------------------------
// Connection loop
// ---------------------------------------------------------------------------

async fn run_connection(
    connection: quinn::Connection,
    local_agent_id: String,
    mut authenticated_peer_id: Option<String>,
    inbound_tx: broadcast::Sender<Envelope>,
    connections: Arc<RwLock<HashMap<String, quinn::Connection>>>,
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
    cancel: CancellationToken,
) {
    let peer_cert_pubkey_b64 = match extract_peer_pubkey_base64_from_connection(&connection) {
        Ok(pubkey) => pubkey,
        Err(err) => {
            warn!(error = %err, "failed to extract peer cert public key");
            return;
        }
    };

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("connection loop shutting down via cancellation");
                break;
            }
            uni = connection.accept_uni() => {
                match uni {
                    Ok(mut recv) => {
                        match read_framed(&mut recv).await {
                            Ok(bytes) => match serde_json::from_slice::<Envelope>(&bytes) {
                                Ok(envelope) => {
                                    if authenticated_peer_id.as_deref() == Some(envelope.from.as_str()) {
                                        let _ = inbound_tx.send(envelope);
                                    } else {
                                        warn!("dropping uni message before hello authentication");
                                    }
                                }
                                Err(err) => {
                                    warn!(error = %err, "dropping malformed uni envelope");
                                }
                            },
                            Err(err) => {
                                warn!(error = %err, "failed reading uni stream");
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            bi = connection.accept_bi() => {
                match bi {
                    Ok((mut send, mut recv)) => {
                        let request = match read_framed(&mut recv).await {
                            Ok(bytes) => match serde_json::from_slice::<Envelope>(&bytes) {
                                Ok(r) => r,
                                Err(err) => {
                                    warn!(error = %err, "dropping malformed bidi envelope");
                                    continue;
                                }
                            },
                            Err(err) => {
                                warn!(error = %err, "failed reading bidi stream");
                                continue;
                            }
                        };

                        let response = if request.kind == MessageKind::Hello {
                            match validate_hello_identity(&request, &peer_cert_pubkey_b64, &expected_pubkeys) {
                                Ok(()) => {
                                    let resp = auto_response(&request, &local_agent_id);
                                    if resp.kind == MessageKind::Hello {
                                        authenticated_peer_id = Some(request.from.clone());
                                        connections
                                            .write()
                                            .await
                                            .insert(request.from.clone(), connection.clone());
                                        let _ = inbound_tx.send(request.clone());
                                    }
                                    resp
                                }
                                Err(err) => Envelope::response_to(
                                    &request,
                                    local_agent_id.clone(),
                                    MessageKind::Error,
                                    json!({
                                        "code": "not_authorized",
                                        "message": err,
                                        "retryable": false
                                    }),
                                ),
                            }
                        } else if authenticated_peer_id.as_deref() != Some(request.from.as_str()) {
                            Envelope::response_to(
                                &request,
                                local_agent_id.clone(),
                                MessageKind::Error,
                                json!({
                                    "code": "not_authorized",
                                    "message": "hello handshake must complete before other requests",
                                    "retryable": false
                                }),
                            )
                        } else {
                            let _ = inbound_tx.send(request.clone());
                            auto_response(&request, &local_agent_id)
                        };

                        if let Ok(response_bytes) = serde_json::to_vec(&response)
                            && write_framed(&mut send, &response_bytes).await.is_ok()
                        {
                            let _ = send.finish().await;
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

// ---------------------------------------------------------------------------
// Hello validation
// ---------------------------------------------------------------------------

fn validate_hello_identity(
    hello: &Envelope,
    cert_pubkey_b64: &str,
    expected_pubkeys: &Arc<StdRwLock<HashMap<String, String>>>,
) -> std::result::Result<(), String> {
    let cert_pubkey_bytes = STANDARD
        .decode(cert_pubkey_b64)
        .map_err(|_| "peer certificate key was not valid base64".to_string())?;
    let derived_agent_id = derive_agent_id_from_pubkey_bytes(&cert_pubkey_bytes);
    if derived_agent_id != hello.from {
        return Err("peer hello 'from' does not match certificate public key identity".to_string());
    }

    let expected = expected_pubkeys
        .read()
        .map_err(|_| "expected peer table lock poisoned".to_string())?;
    if let Some(expected_pubkey) = expected.get(&hello.from)
        && expected_pubkey != cert_pubkey_b64
    {
        return Err("peer certificate public key does not match discovered key".to_string());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Auto-response for incoming requests
// ---------------------------------------------------------------------------

pub fn auto_response(request: &Envelope, local_agent_id: &str) -> Envelope {
    match request.kind {
        MessageKind::Hello => {
            if !hello_request_supports_protocol_v1(request) {
                return Envelope::response_to(
                    request,
                    local_agent_id.to_string(),
                    MessageKind::Error,
                    json!({
                        "code": "incompatible_version",
                        "message": format!(
                            "no mutually supported protocol version. This agent supports: [1]. \
                             Received: {:?}",
                            request.payload.get("protocol_versions")
                        ),
                        "retryable": false
                    }),
                );
            }

            let payload = json!({
                "protocol_versions": [1],
                "selected_version": 1,
                "features": hello_features(),
            });
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
            json!({"status": "idle", "uptime_secs": 0, "active_tasks": 0}),
        ),
        MessageKind::Discover => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Capabilities,
            json!({
                "agent_name": "AXON Agent",
                "domains": ["meta.status"],
                "tools": ["axon"],
                "max_concurrent_tasks": 1
            }),
        ),
        MessageKind::Query => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Response,
            json!({
                "data": {"accepted": true},
                "summary": "Query received",
                "tokens_used": 0,
                "truncated": false
            }),
        ),
        MessageKind::Delegate | MessageKind::Cancel => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Ack,
            json!({"accepted": true}),
        ),
        _ => Envelope::response_to(
            request,
            local_agent_id.to_string(),
            MessageKind::Error,
            json!({
                "code": "unknown_kind",
                "message": format!(
                    "unsupported request kind '{}' for bidirectional stream",
                    request.kind
                ),
                "retryable": false
            }),
        ),
    }
}

fn hello_request_supports_protocol_v1(hello: &Envelope) -> bool {
    hello
        .payload
        .get("protocol_versions")
        .and_then(|v| v.as_array())
        .map(|versions| versions.iter().any(|v| v.as_u64() == Some(1)))
        .unwrap_or(false)
}

/// Check whether a hello response selected protocol version 1.
/// Used by tests and internally for version negotiation.
#[cfg(test)]
fn hello_selected_version_is_supported(hello_response: &Envelope) -> bool {
    hello_response
        .payload
        .get("selected_version")
        .and_then(|v| v.as_u64())
        == Some(1)
}

// ---------------------------------------------------------------------------
// Wire format: length-prefixed framing
// ---------------------------------------------------------------------------

async fn send_unidirectional(
    connection: &quinn::Connection,
    envelope: Envelope,
) -> Result<()> {
    let bytes = serde_json::to_vec(&envelope).context("failed to serialize envelope")?;
    if bytes.len() > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!("message exceeds max size {MAX_MESSAGE_SIZE} bytes"));
    }

    let mut stream = connection
        .open_uni()
        .await
        .context("failed to open uni stream")?;
    write_framed(&mut stream, &bytes).await?;
    stream
        .finish()
        .await
        .context("failed to finish uni stream")?;
    Ok(())
}

async fn send_request(
    connection: &quinn::Connection,
    envelope: Envelope,
) -> Result<Envelope> {
    let bytes = serde_json::to_vec(&envelope).context("failed to serialize request")?;
    if bytes.len() > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!("message exceeds max size {MAX_MESSAGE_SIZE} bytes"));
    }

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("failed to open bidi stream")?;
    write_framed(&mut send, &bytes).await?;
    send.finish()
        .await
        .context("failed to finish request stream")?;

    let response_bytes = timeout(REQUEST_TIMEOUT, read_framed(&mut recv))
        .await
        .context("request timed out after 30s")??;
    let response = serde_json::from_slice::<Envelope>(&response_bytes)
        .context("failed to decode response envelope")?;
    Ok(response)
}

async fn write_framed(stream: &mut quinn::SendStream, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!("message too large for framing"));
    }

    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
    stream
        .write_all(&buf)
        .await
        .context("failed to write framed payload")?;
    Ok(())
}

async fn read_framed(stream: &mut quinn::RecvStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .context("failed to read frame length")?;

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!(
            "declared frame length {len} exceeds max message size {MAX_MESSAGE_SIZE}"
        ));
    }

    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .context("failed to read frame body")?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Certificate helpers
// ---------------------------------------------------------------------------

fn extract_peer_pubkey_base64_from_connection(connection: &quinn::Connection) -> Result<String> {
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

pub fn extract_ed25519_pubkey_from_cert_der(cert_der: &[u8]) -> Result<[u8; 32]> {
    let (_remaining, cert) = parse_x509_certificate(cert_der)
        .map_err(|err| anyhow!("failed to parse certificate DER: {err}"))?;

    let key = cert.public_key().subject_public_key.data.as_ref();
    if key.len() != 32 {
        return Err(anyhow!(
            "unexpected public key length {}; expected 32 bytes Ed25519",
            key.len()
        ));
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(key);
    Ok(out)
}

fn extract_subject_dn_from_cert_der(cert_der: &[u8]) -> Result<Vec<u8>> {
    let (_remaining, cert) = parse_x509_certificate(cert_der)
        .map_err(|err| anyhow!("failed to parse certificate DER: {err}"))?;
    Ok(cert.tbs_certificate.subject.as_raw().to_vec())
}

fn derive_agent_id_from_pubkey_bytes(pubkey: &[u8]) -> String {
    let digest = Sha256::digest(pubkey);
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// TLS verifiers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PeerCertVerifier {
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
}

#[derive(Debug)]
struct PeerClientCertVerifier {
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
    roots: Vec<DistinguishedName>,
}

impl ServerCertVerifier for PeerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: SystemTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let expected_agent_id = match server_name {
            rustls::ServerName::DnsName(name) => name.as_ref().to_string(),
            _ => {
                return Err(rustls::Error::General(
                    "unsupported server name type for AXON peer verification".to_string(),
                ));
            }
        };

        let cert_key = extract_ed25519_pubkey_from_cert_der(&end_entity.0).map_err(|err| {
            rustls::Error::General(format!("failed parsing server cert key: {err}"))
        })?;
        let cert_key_b64 = STANDARD.encode(cert_key);
        let derived_agent_id = derive_agent_id_from_pubkey_bytes(&cert_key);

        if derived_agent_id != expected_agent_id {
            return Err(rustls::Error::General(
                "server cert public key does not match expected agent_id".to_string(),
            ));
        }

        // std::sync required: rustls verifier callbacks are synchronous
        let expected = self
            .expected_pubkeys
            .read()
            .map_err(|_| rustls::Error::General("expected peer table lock poisoned".to_string()))?;
        if let Some(expected_pubkey_b64) = expected.get(&expected_agent_id) {
            if cert_key_b64 != *expected_pubkey_b64 {
                return Err(rustls::Error::General(
                    "server cert public key mismatch against discovery data".to_string(),
                ));
            }
        } else {
            warn!(agent_id = %expected_agent_id, "accepting unknown server peer via TOFU — no prior pubkey on record");
        }

        Ok(ServerCertVerified::assertion())
    }
}

impl ClientCertVerifier for PeerClientCertVerifier {
    fn client_auth_root_subjects(&self) -> &[DistinguishedName] {
        &self.roots
    }

    fn verify_client_cert(
        &self,
        end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _now: SystemTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        let cert_key = extract_ed25519_pubkey_from_cert_der(&end_entity.0).map_err(|err| {
            rustls::Error::General(format!("failed parsing client cert key: {err}"))
        })?;
        let cert_pubkey_b64 = STANDARD.encode(cert_key);
        let agent_id = derive_agent_id_from_pubkey_bytes(&cert_key);

        // std::sync required: rustls verifier callbacks are synchronous
        let expected = self
            .expected_pubkeys
            .read()
            .map_err(|_| rustls::Error::General("expected peer table lock poisoned".to_string()))?;
        if let Some(expected_pubkey_b64) = expected.get(&agent_id) {
            if &cert_pubkey_b64 != expected_pubkey_b64 {
                return Err(rustls::Error::General(
                    "client cert public key does not match discovered peer key".to_string(),
                ));
            }
        } else {
            warn!(agent_id = %agent_id, "accepting unknown client peer via TOFU — no prior pubkey on record");
        }

        Ok(ClientCertVerified::assertion())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AxonPaths;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn agent_a() -> String {
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string()
    }

    fn agent_b() -> String {
        "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
    }

    #[test]
    fn auto_response_hello_success() {
        let req = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({"protocol_versions": [1], "features": ["delegate"]}),
        );
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Hello);
        assert_eq!(resp.ref_id, Some(req.id));
        assert_eq!(resp.payload["selected_version"], 1);
        assert!(resp.payload.get("pubkey").is_none());
    }

    #[test]
    fn auto_response_hello_incompatible_version() {
        let req = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({"protocol_versions": [2]}),
        );
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Error);
        assert_eq!(
            resp.payload.get("code").and_then(|v| v.as_str()),
            Some("incompatible_version")
        );
    }

    #[test]
    fn auto_response_ping() {
        let req = Envelope::new(agent_a(), agent_b(), MessageKind::Ping, json!({}));
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Pong);
        assert_eq!(resp.ref_id, Some(req.id));
    }

    #[test]
    fn auto_response_discover() {
        let req = Envelope::new(agent_a(), agent_b(), MessageKind::Discover, json!({}));
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Capabilities);
    }

    #[test]
    fn auto_response_query() {
        let req = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Query,
            json!({"question": "test?"}),
        );
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Response);
    }

    #[test]
    fn auto_response_delegate() {
        let req = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Delegate,
            json!({"task": "do something"}),
        );
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Ack);
    }

    #[test]
    fn auto_response_cancel() {
        let req = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Cancel,
            json!({"reason": "changed mind"}),
        );
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Ack);
    }

    #[test]
    fn auto_response_unknown_kind() {
        let req = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Pong,
            json!({}),
        );
        let resp = auto_response(&req, &agent_b());
        assert_eq!(resp.kind, MessageKind::Error);
        assert_eq!(
            resp.payload.get("code").and_then(|v| v.as_str()),
            Some("unknown_kind")
        );
    }

    #[test]
    fn hello_selected_version_parser() {
        let ok = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({"selected_version": 1}),
        );
        let bad = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({"selected_version": 2}),
        );
        let missing = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({}),
        );
        assert!(hello_selected_version_is_supported(&ok));
        assert!(!hello_selected_version_is_supported(&bad));
        assert!(!hello_selected_version_is_supported(&missing));
    }

    #[test]
    fn hello_v1_protocol_check() {
        let yes = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({"protocol_versions": [1]}),
        );
        let multi = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({"protocol_versions": [1, 2]}),
        );
        let no = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({"protocol_versions": [2, 3]}),
        );
        let empty = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Hello,
            json!({}),
        );

        assert!(hello_request_supports_protocol_v1(&yes));
        assert!(hello_request_supports_protocol_v1(&multi));
        assert!(!hello_request_supports_protocol_v1(&no));
        assert!(!hello_request_supports_protocol_v1(&empty));
    }

    #[test]
    fn cert_pubkey_extraction_matches_identity() {
        let dir = tempdir().expect("tempdir");
        let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
        let identity = Identity::load_or_generate(&paths).expect("identity");
        let cert = identity.make_quic_certificate().expect("cert");

        let extracted =
            extract_ed25519_pubkey_from_cert_der(&cert.cert_der).expect("extract pubkey");
        let cert_pubkey_b64 = STANDARD.encode(extracted);

        assert_eq!(cert_pubkey_b64, identity.public_key_base64());
        assert_eq!(
            derive_agent_id_from_pubkey_bytes(&extracted),
            identity.agent_id()
        );
    }

    #[test]
    fn derive_agent_id_deterministic() {
        let key = [42u8; 32];
        let id1 = derive_agent_id_from_pubkey_bytes(&key);
        let id2 = derive_agent_id_from_pubkey_bytes(&key);
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 32);
    }

    #[test]
    fn transport_constants() {
        assert_eq!(KEEPALIVE_INTERVAL, Duration::from_secs(15));
        assert_eq!(IDLE_TIMEOUT, Duration::from_secs(60));
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn endpoint_binds_and_reports_addr() {
        let dir = tempdir().expect("tempdir");
        let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
        let identity = Identity::load_or_generate(&paths).expect("identity");

        let transport = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &identity)
            .await
            .expect("bind");

        let addr = transport.local_addr().expect("local_addr");
        assert_eq!(addr.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
        assert_ne!(addr.port(), 0);
    }

    #[tokio::test]
    async fn two_peers_hello_exchange() {
        let dir_a = tempdir().expect("tempdir a");
        let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
        let id_a = Identity::load_or_generate(&paths_a).expect("identity a");

        let dir_b = tempdir().expect("tempdir b");
        let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
        let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

        let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b)
            .await
            .expect("bind b");
        let addr_b = transport_b.local_addr().expect("local_addr b");

        let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a)
            .await
            .expect("bind a");

        let peer_b = PeerRecord {
            agent_id: id_b.agent_id().to_string(),
            addr: addr_b,
            pubkey: id_b.public_key_base64().to_string(),
            source: crate::peer_table::PeerSource::Static,
            status: crate::peer_table::ConnectionStatus::Discovered,
            rtt_ms: None,
            last_seen: std::time::Instant::now(),
        };

        transport_a.set_expected_peer(id_b.agent_id().to_string(), id_b.public_key_base64().to_string());
        transport_b.set_expected_peer(id_a.agent_id().to_string(), id_a.public_key_base64().to_string());

        let conn = transport_a.ensure_connection(&peer_b).await.expect("connect");
        assert!(conn.close_reason().is_none());
        assert!(transport_a.has_connection(id_b.agent_id()).await);
    }

    #[tokio::test]
    async fn send_notify_unidirectional() {
        let dir_a = tempdir().expect("tempdir a");
        let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
        let id_a = Identity::load_or_generate(&paths_a).expect("identity a");

        let dir_b = tempdir().expect("tempdir b");
        let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
        let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

        let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b)
            .await
            .expect("bind b");
        let addr_b = transport_b.local_addr().expect("local_addr b");
        let mut rx_b = transport_b.subscribe_inbound();

        let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a)
            .await
            .expect("bind a");

        transport_a.set_expected_peer(id_b.agent_id().to_string(), id_b.public_key_base64().to_string());
        transport_b.set_expected_peer(id_a.agent_id().to_string(), id_a.public_key_base64().to_string());

        let peer_b = PeerRecord {
            agent_id: id_b.agent_id().to_string(),
            addr: addr_b,
            pubkey: id_b.public_key_base64().to_string(),
            source: crate::peer_table::PeerSource::Static,
            status: crate::peer_table::ConnectionStatus::Discovered,
            rtt_ms: None,
            last_seen: std::time::Instant::now(),
        };

        let notify = Envelope::new(
            id_a.agent_id().to_string(),
            id_b.agent_id().to_string(),
            MessageKind::Notify,
            json!({"topic": "test", "data": {"msg": "hello"}, "importance": "low"}),
        );

        let result = transport_a.send(&peer_b, notify.clone()).await.expect("send");
        assert!(result.is_none());

        // Drain until we find the notify (hello is also broadcast)
        let received = loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
                .await
                .expect("timeout waiting for inbound")
                .expect("recv");
            if msg.kind != MessageKind::Hello {
                break msg;
            }
        };
        assert_eq!(received.kind, MessageKind::Notify);
        assert_eq!(received.from, id_a.agent_id());
    }

    #[tokio::test]
    async fn send_ping_bidirectional() {
        let dir_a = tempdir().expect("tempdir a");
        let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
        let id_a = Identity::load_or_generate(&paths_a).expect("identity a");

        let dir_b = tempdir().expect("tempdir b");
        let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
        let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

        let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b)
            .await
            .expect("bind b");
        let addr_b = transport_b.local_addr().expect("local_addr b");

        let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a)
            .await
            .expect("bind a");

        transport_a.set_expected_peer(id_b.agent_id().to_string(), id_b.public_key_base64().to_string());
        transport_b.set_expected_peer(id_a.agent_id().to_string(), id_a.public_key_base64().to_string());

        let peer_b = PeerRecord {
            agent_id: id_b.agent_id().to_string(),
            addr: addr_b,
            pubkey: id_b.public_key_base64().to_string(),
            source: crate::peer_table::PeerSource::Static,
            status: crate::peer_table::ConnectionStatus::Discovered,
            rtt_ms: None,
            last_seen: std::time::Instant::now(),
        };

        let ping = Envelope::new(
            id_a.agent_id().to_string(),
            id_b.agent_id().to_string(),
            MessageKind::Ping,
            json!({}),
        );

        let result = transport_a.send(&peer_b, ping.clone()).await.expect("send");
        let response = result.expect("expected response");
        assert_eq!(response.kind, MessageKind::Pong);
        assert_eq!(response.ref_id, Some(ping.id));
    }
}
