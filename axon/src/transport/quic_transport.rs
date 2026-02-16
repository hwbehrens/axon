use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::identity::Identity;
use crate::message::{AgentId, Envelope};
use crate::peer_table::PeerRecord;

use super::connection::run_connection;
use super::framing::{send_request, send_unidirectional};
use super::tls::build_endpoint;

/// Optional callback to produce a response for a bidirectional request.
/// If `None` is returned, the default `auto_response` is used.
pub type ResponseHandlerFn = Arc<
    dyn Fn(Arc<Envelope>) -> Pin<Box<dyn Future<Output = Option<Envelope>> + Send>> + Send + Sync,
>;

#[derive(Clone)]
pub struct QuicTransport {
    endpoint: quinn::Endpoint,
    local_agent_id: AgentId,
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
    response_handler: Option<ResponseHandlerFn>,
    inbound_read_timeout: Duration,
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
            Duration::from_secs(10),
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
        response_handler: Option<ResponseHandlerFn>,
        inbound_read_timeout: Duration,
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
            local_agent_id: AgentId::from(identity.agent_id()),
            max_connections,
            connections: Arc::new(RwLock::new(HashMap::new())),
            connecting_locks: Arc::new(RwLock::new(HashMap::new())),
            expected_pubkeys,
            inbound_tx,
            inbound_semaphore: Arc::new(Semaphore::new(max_connections)),
            cancel,
            response_handler,
            inbound_read_timeout,
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

    /// Remove stale per-peer connecting lock entries for peers that are no
    /// longer expected (not in `expected_pubkeys`). Called periodically to
    /// prevent unbounded growth from transient mDNS peers.
    pub async fn gc_connecting_locks(&self) {
        let expected: Vec<String> = self
            .expected_pubkeys
            .read()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let mut locks = self.connecting_locks.write().await;
        locks.retain(|k, _| expected.contains(k));
    }

    pub async fn has_connection(&self, agent_id: &str) -> bool {
        self.connections.read().await.contains_key(agent_id)
    }

    pub async fn ensure_connection(&self, peer: &PeerRecord) -> Result<quinn::Connection> {
        self.set_expected_peer(peer.agent_id.to_string(), peer.pubkey.clone());

        // Fast path: already connected.
        if let Some(existing) = self
            .connections
            .read()
            .await
            .get(peer.agent_id.as_str())
            .cloned()
        {
            return Ok(existing);
        }

        // Acquire per-peer lock to prevent duplicate concurrent connection attempts.
        let peer_lock = {
            let mut locks = self.connecting_locks.write().await;
            locks
                .entry(peer.agent_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = peer_lock.lock().await;

        // Re-check after acquiring the lock — another task may have connected.
        if let Some(existing) = self
            .connections
            .read()
            .await
            .get(peer.agent_id.as_str())
            .cloned()
        {
            return Ok(existing);
        }

        let connecting = self
            .endpoint
            .connect(peer.addr, &peer.agent_id)
            .with_context(|| format!("failed to begin QUIC connect to {}", peer.addr))?;

        let connection = connecting
            .await
            .with_context(|| format!("QUIC handshake failed with {}", peer.addr))?;

        self.connections
            .write()
            .await
            .insert(peer.agent_id.to_string(), connection.clone());
        self.spawn_connection_loop(connection.clone());

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

    fn spawn_accept_loop(&self) {
        let endpoint = self.endpoint.clone();
        let inbound_tx = self.inbound_tx.clone();
        let local_id = self.local_agent_id.clone();
        let connections = self.connections.clone();
        let cancel = self.cancel.clone();
        let max_connections = self.max_connections;
        let inbound_semaphore = self.inbound_semaphore.clone();
        let response_handler = self.response_handler.clone();
        let inbound_read_timeout = self.inbound_read_timeout;

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
                                let cancel = cancel.clone();
                                let response_handler = response_handler.clone();
                                tokio::spawn(async move {
                                    run_connection(
                                        connection,
                                        local_id.to_string(),
                                        inbound_tx,
                                        connections,
                                        cancel,
                                        response_handler,
                                        inbound_read_timeout,
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

    fn spawn_connection_loop(&self, connection: quinn::Connection) {
        let inbound_tx = self.inbound_tx.clone();
        let local_id = self.local_agent_id.clone();
        let connections = self.connections.clone();
        let cancel = self.cancel.clone();
        let response_handler = self.response_handler.clone();
        let inbound_read_timeout = self.inbound_read_timeout;

        tokio::spawn(async move {
            run_connection(
                connection,
                local_id.to_string(),
                inbound_tx,
                connections,
                cancel,
                response_handler,
                inbound_read_timeout,
            )
            .await;
        });
    }
}

#[cfg(test)]
#[path = "quic_transport_tests.rs"]
mod tests;
