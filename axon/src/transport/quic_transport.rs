use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::identity::Identity;
use crate::message::{Envelope, HelloPayload, MessageKind, hello_features};
use crate::peer_table::PeerRecord;

use super::connection::run_connection;
use super::framing::{send_request, send_unidirectional};
use super::tls::build_endpoint;

/// Optional callback to check if a message is a replay. Returns true if replay (should drop).
pub type ReplayCheckFn =
    Arc<dyn Fn(uuid::Uuid) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// Optional callback to produce a response for a bidirectional request.
/// If `None` is returned, the default `auto_response` is used.
pub type ResponseHandlerFn = Arc<
    dyn Fn(Arc<Envelope>) -> Pin<Box<dyn Future<Output = Option<Envelope>> + Send>> + Send + Sync,
>;

#[derive(Clone)]
pub struct QuicTransport {
    endpoint: quinn::Endpoint,
    local_agent_id: String,
    max_connections: usize,
    connections: Arc<RwLock<HashMap<String, quinn::Connection>>>,
    /// Per-peer lock to prevent concurrent connection attempts to the same peer.
    connecting_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    /// Peer public key map shared with TLS verifier callbacks.
    ///
    /// Uses `std::sync::RwLock` (not `tokio::sync`) because rustls `ServerCertVerifier`
    /// and `ClientCertVerifier` callbacks are synchronous — they cannot `.await`.
    /// All access is non-blocking: short inserts/removes via `set_expected_peer` /
    /// `remove_expected_peer`, and short reads inside TLS verification.
    /// Do NOT hold this lock across `.await` points.
    expected_pubkeys: Arc<StdRwLock<HashMap<String, String>>>,
    inbound_tx: broadcast::Sender<Arc<Envelope>>,
    inbound_semaphore: Arc<Semaphore>,
    cancel: CancellationToken,
    replay_check: Option<ReplayCheckFn>,
    response_handler: Option<ResponseHandlerFn>,
}

impl QuicTransport {
    pub async fn bind(
        bind_addr: SocketAddr,
        identity: &Identity,
        max_connections: usize,
    ) -> Result<Self> {
        Self::bind_cancellable(
            bind_addr,
            identity,
            CancellationToken::new(),
            max_connections,
            Duration::from_secs(15),
            Duration::from_secs(60),
            None,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn bind_cancellable(
        bind_addr: SocketAddr,
        identity: &Identity,
        cancel: CancellationToken,
        max_connections: usize,
        keepalive: Duration,
        idle_timeout: Duration,
        replay_check: Option<ReplayCheckFn>,
        response_handler: Option<ResponseHandlerFn>,
    ) -> Result<Self> {
        let cert = identity.make_quic_certificate()?;
        let expected_pubkeys = Arc::new(StdRwLock::new(HashMap::new()));
        let (endpoint, inbound_tx) = build_endpoint(
            bind_addr,
            &cert,
            expected_pubkeys.clone(),
            keepalive,
            idle_timeout,
        )?;

        let transport = Self {
            endpoint,
            local_agent_id: identity.agent_id().to_string(),
            max_connections,
            connections: Arc::new(RwLock::new(HashMap::new())),
            connecting_locks: Arc::new(RwLock::new(HashMap::new())),
            expected_pubkeys,
            inbound_tx,
            inbound_semaphore: Arc::new(Semaphore::new(max_connections)),
            cancel,
            replay_check,
            response_handler,
        };
        transport.spawn_accept_loop();
        Ok(transport)
    }

    pub fn subscribe_inbound(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.inbound_tx.subscribe()
    }

    pub fn set_expected_peer(&self, agent_id: String, pubkey: String) {
        if let Ok(mut map) = self.expected_pubkeys.write() {
            map.insert(agent_id, pubkey);
        }
    }

    pub fn remove_expected_peer(&self, agent_id: &str) {
        if let Ok(mut map) = self.expected_pubkeys.write() {
            map.remove(agent_id);
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
        let hello_payload = HelloPayload {
            protocol_versions: vec![1],
            selected_version: None,
            agent_name: None,
            features: hello_features(),
        };

        let hello = Envelope::new(
            self.local_agent_id.clone(),
            remote_agent_id.to_string(),
            MessageKind::Hello,
            serde_json::to_value(hello_payload).context("failed to encode hello payload")?,
        );

        let response = send_request(connection, hello).await?;
        match response.kind {
            MessageKind::Hello => {
                let payload = response.payload_value();
                let selected = payload.get("selected_version").and_then(|v| v.as_u64());
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
                let payload = response.payload_value();
                let code = payload
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown_error");
                let message = payload
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
        let max_connections = self.max_connections;
        let inbound_semaphore = self.inbound_semaphore.clone();
        let replay_check = self.replay_check.clone();
        let response_handler = self.response_handler.clone();

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
                                let permit = match inbound_semaphore.clone().try_acquire_owned() {
                                    Ok(permit) => permit,
                                    Err(_) => {
                                        warn!(
                                            max = max_connections,
                                            "rejecting inbound QUIC connection: connection limit reached"
                                        );
                                        connection.close(0u32.into(), b"connection limit reached");
                                        continue;
                                    }
                                };
                                debug!(remote = ?connection.remote_address(), "accepted inbound QUIC connection");
                                let inbound_tx = inbound_tx.clone();
                                let local_id = local_id.clone();
                                let connections = connections.clone();
                                let expected_pubkeys = expected_pubkeys.clone();
                                let cancel = cancel.clone();
                                let replay_check = replay_check.clone();
                                let response_handler = response_handler.clone();
                                tokio::spawn(async move {
                                    run_connection(
                                        connection,
                                        local_id,
                                        None,
                                        inbound_tx,
                                        connections,
                                        expected_pubkeys,
                                        cancel,
                                        replay_check,
                                        response_handler,
                                    )
                                    .await;
                                    drop(permit);
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
        let replay_check = self.replay_check.clone();
        let response_handler = self.response_handler.clone();

        tokio::spawn(async move {
            run_connection(
                connection,
                local_id,
                initial_authenticated_peer_id,
                inbound_tx,
                connections,
                expected_pubkeys,
                cancel,
                replay_check,
                response_handler,
            )
            .await;
        });
    }
}

#[cfg(test)]
#[path = "quic_transport_tests.rs"]
mod tests;
