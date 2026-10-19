use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::UnixListener;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::auth;
use super::bind::bind_socket;
use super::client_handler::handle_client;
use super::protocol::{
    CommandEvent, DaemonReply, EncodeLineError, IpcErrorCode, WhoamiInfo, encode_reply_line,
    error_reply_line,
};
use crate::message::Envelope;

#[path = "request_event.rs"]
mod request_event;
pub use request_event::RequestEventError;

#[derive(Clone)]
struct ClientHandle {
    tx: mpsc::Sender<Arc<str>>,
    cancel: CancellationToken,
}

pub struct IpcServerConfig {
    pub agent_id: String,
    pub public_key: String,
    pub name: Option<String>,
    pub version: String,
    pub max_client_queue: usize,
    pub uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for IpcServerConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            public_key: String::new(),
            name: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
            max_client_queue: 1024,
            uptime_secs: Arc::new(|| 0),
        }
    }
}

/// Unix domain socket IPC server that bridges local clients to the AXON daemon.
/// Handles connection accept, per-client read/write loops, and command dispatch.
#[derive(Clone)]
pub struct IpcServer {
    socket_path: PathBuf,
    max_clients: usize,
    clients: Arc<Mutex<HashMap<u64, ClientHandle>>>,
    next_client_id: Arc<AtomicU64>,
    owner_uid: u32,
    max_client_queue: usize,
    config: Arc<IpcServerConfig>,
    disconnected_tx: broadcast::Sender<u64>,
    cancel: CancellationToken,
    tasks: TaskTracker,
}

impl IpcServer {
    pub async fn bind(
        socket_path: PathBuf,
        max_clients: usize,
        config: IpcServerConfig,
    ) -> Result<(Self, mpsc::Receiver<CommandEvent>)> {
        if socket_path.exists() {
            let meta = tokio::fs::symlink_metadata(&socket_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to read metadata for socket path: {}",
                        socket_path.display()
                    )
                })?;
            if !meta.file_type().is_socket() {
                anyhow::bail!(
                    "refusing to remove non-socket file at socket path: {}",
                    socket_path.display()
                );
            }
            tokio::fs::remove_file(&socket_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to remove stale unix socket: {}",
                        socket_path.display()
                    )
                })?;
        }

        // Bind inside a private staging dir, then rename into place. The
        // final path must never exist with a umask-derived mode between bind
        // and chmod (spec/IPC.md §2 requires 0600); `bind_socket` publishes
        // it already chmodded and creates missing parents.
        let listener = bind_socket(&socket_path)?;

        let owner_uid = unsafe { libc::getuid() };
        let max_client_queue = config.max_client_queue;

        let (disconnected_tx, _) = broadcast::channel(256);
        let server = Self {
            socket_path,
            max_clients,
            clients: Arc::new(Mutex::new(HashMap::new())),
            next_client_id: Arc::new(AtomicU64::new(1)),
            owner_uid,
            max_client_queue,
            config: Arc::new(config),
            disconnected_tx,
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
        };

        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        server.start_accept_loop(listener, cmd_tx);

        Ok((server, cmd_rx))
    }

    pub async fn send_reply(&self, client_id: u64, reply: &DaemonReply) -> Result<()> {
        // Every outbound line passes one encoder that enforces the framed
        // limit (newline included). An oversized reply fails EXPLICITLY:
        // the client receives a `message_too_large` error carrying the same
        // req_id — never a truncated payload, never a panic.
        let line = match encode_reply_line(reply) {
            Ok(line) => line,
            Err(EncodeLineError::TooLarge(bytes)) => {
                tracing::warn!(
                    client_id,
                    bytes,
                    "IPC reply exceeds the line limit; delivering message_too_large instead"
                );
                // `error_reply_line` drops the req_id echo rather than
                // exceed the limit: a pathological req_id (bounded at
                // ingress, but defended here anyway) can never make the
                // fallback itself unframeable.
                error_reply_line(
                    IpcErrorCode::MessageTooLarge,
                    reply.req_id().map(str::to_string),
                )
                .map_err(|err| anyhow::anyhow!("failed to encode error reply: {err:?}"))?
            }
            Err(EncodeLineError::Serialize(err)) => {
                anyhow::bail!("failed to serialize daemon reply: {err}")
            }
        };
        let tx = {
            let clients = self.clients.lock().await;
            clients.get(&client_id).map(|client| client.tx.clone())
        };
        if let Some(tx) = tx {
            // Non-blocking: control commands send their replies from the
            // daemon's control loop, so awaiting queue capacity here would
            // let one non-reading client stall every other client's
            // commands (I7). Queue overflow disconnects the lagging client
            // instead of silently dropping the reply (spec/IPC.md §7), the
            // same policy broadcasts already use.
            if tx.try_send(line).is_err() {
                self.close_client(client_id).await;
            }
        }
        Ok(())
    }

    pub async fn broadcast_inbound(&self, envelope: &Envelope) -> Result<()> {
        let event = DaemonReply::InboundEvent {
            event: "inbound",
            from: envelope
                .from
                .as_ref()
                .map(|id| id.to_string())
                .unwrap_or_default(),
            envelope: envelope.clone(),
        };
        // Oversized broadcasts are dropped with a warning, never truncated:
        // a near-limit network envelope legitimately grows past the IPC
        // line limit once wrapped in event JSON.
        let line = match encode_reply_line(&event) {
            Ok(line) => line,
            Err(EncodeLineError::TooLarge(bytes)) => {
                tracing::warn!(
                    bytes,
                    msg_id = %envelope.id,
                    "inbound envelope exceeds the IPC line limit when framed; dropping the event"
                );
                return Ok(());
            }
            Err(EncodeLineError::Serialize(err)) => {
                anyhow::bail!("failed to serialize inbound event: {err}")
            }
        };
        self.broadcast_line(line).await
    }

    pub fn subscribe_disconnects(&self) -> broadcast::Receiver<u64> {
        self.disconnected_tx.subscribe()
    }

    pub async fn broadcast_peer_candidate(
        &self,
        agent_id: &str,
        public_key: &str,
        locators: Vec<String>,
        source: &'static str,
    ) -> Result<()> {
        let event = DaemonReply::PeerCandidateEvent {
            event: "peer_candidate",
            agent_id: agent_id.to_string(),
            public_key: public_key.to_string(),
            locators,
            source,
        };
        // Same rule as inbound broadcasts: drop with a warning, never
        // truncate. (Candidate events are small by construction; the check
        // exists so no outbound path can bypass the framed limit.)
        let line = match encode_reply_line(&event) {
            Ok(line) => line,
            Err(EncodeLineError::TooLarge(bytes)) => {
                tracing::warn!(
                    bytes,
                    "peer-candidate event exceeds the IPC line limit; dropping the event"
                );
                return Ok(());
            }
            Err(EncodeLineError::Serialize(err)) => {
                anyhow::bail!("failed to serialize peer-candidate event: {err}")
            }
        };
        self.broadcast_line(line).await
    }

    /// Identity metadata served by the `whoami` command. The daemon's
    /// command handler composes the reply from this; `IpcServer` owns the
    /// config the reply is built from but does not dispatch commands.
    pub fn whoami_info(&self) -> WhoamiInfo {
        WhoamiInfo {
            agent_id: self.config.agent_id.clone(),
            public_key: self.config.public_key.clone(),
            name: self.config.name.clone(),
            version: self.config.version.clone(),
            uptime_secs: (self.config.uptime_secs)(),
        }
    }

    /// Close a client connection by removing it from the client map and
    /// signaling cancellation to terminate its read/write loops.
    pub async fn close_client(&self, client_id: u64) {
        if let Some(client) = self.clients.lock().await.remove(&client_id) {
            client.cancel.cancel();
            let _ = self.disconnected_tx.send(client_id);
        }
    }

    pub async fn client_count(&self) -> usize {
        self.clients.lock().await.len()
    }

    /// Ids of currently connected clients, for reconciling state that is
    /// keyed by client when disconnect notifications are lost.
    pub async fn connected_client_ids(&self) -> std::collections::HashSet<u64> {
        self.clients.lock().await.keys().copied().collect()
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn cleanup_socket(&self) -> Result<()> {
        if self.socket_path.exists() {
            fs::remove_file(&self.socket_path).with_context(|| {
                format!(
                    "failed to remove socket file: {}",
                    self.socket_path.display()
                )
            })?;
        }
        Ok(())
    }

    /// Stop accepting clients, cancel every active handler, wait for owned
    /// tasks up to the shutdown deadline, and remove the socket path.
    pub async fn shutdown(&self) -> Result<()> {
        self.cancel.cancel();
        let clients = std::mem::take(&mut *self.clients.lock().await);
        for (client_id, client) in clients {
            client.cancel.cancel();
            let _ = self.disconnected_tx.send(client_id);
        }
        self.tasks.close();
        if tokio::time::timeout(std::time::Duration::from_secs(2), self.tasks.wait())
            .await
            .is_err()
        {
            tracing::warn!("timed out waiting for IPC tasks to stop");
        }
        self.cleanup_socket()
    }

    async fn broadcast_line(&self, line: Arc<str>) -> Result<()> {
        let mut clients = self.clients.lock().await;
        let mut disconnected = Vec::new();
        for (client_id, client) in clients.iter() {
            if client.tx.try_send(line.clone()).is_err() {
                disconnected.push(*client_id);
            }
        }
        for client_id in disconnected {
            if let Some(client) = clients.remove(&client_id) {
                client.cancel.cancel();
                let _ = self.disconnected_tx.send(client_id);
            }
        }
        Ok(())
    }

    fn start_accept_loop(&self, listener: UnixListener, cmd_tx: mpsc::Sender<CommandEvent>) {
        let clients = self.clients.clone();
        let next_client_id = self.next_client_id.clone();
        let max_clients = self.max_clients;
        let owner_uid = self.owner_uid;
        let max_client_queue = self.max_client_queue;
        let disconnected_tx = self.disconnected_tx.clone();
        let server_cancel = self.cancel.clone();
        let tasks = self.tasks.clone();

        self.tasks.spawn(async move {
            let mut accept_failures: u32 = 0;
            loop {
                let accepted = tokio::select! {
                    _ = server_cancel.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                let (socket, _) = match accepted {
                    Ok(v) => {
                        accept_failures = 0;
                        v
                    }
                    Err(err) => {
                        // Persistent accept errors (e.g. EMFILE) must not
                        // spin the loop at 100% CPU; back off, bounded, and
                        // stay cancellable for shutdown.
                        accept_failures = accept_failures.saturating_add(1);
                        tracing::warn!(
                            error = %err,
                            failures = accept_failures,
                            "failed to accept IPC connection"
                        );
                        tokio::select! {
                            _ = server_cancel.cancelled() => break,
                            _ = tokio::time::sleep(accept_backoff(accept_failures)) => {}
                        }
                        continue;
                    }
                };
                if server_cancel.is_cancelled() {
                    break;
                }

                let client_id = next_client_id.fetch_add(1, Ordering::Relaxed);

                {
                    let clients_guard = clients.lock().await;
                    if clients_guard.len() >= max_clients {
                        tracing::warn!(
                            max = max_clients,
                            "rejecting IPC connection: client limit reached"
                        );
                        drop(clients_guard);
                        drop(socket);
                        continue;
                    }
                }

                // Check peer credentials for implicit authentication.
                // Reject clients that do not match the daemon owner's UID.
                let peer_uid = auth::peer_uid(&socket);
                let is_owner_uid = matches!(peer_uid, Some(uid) if uid == owner_uid);
                if !is_owner_uid {
                    tracing::warn!(
                        client_id,
                        owner_uid,
                        peer_uid = ?peer_uid,
                        "rejecting IPC connection: peer UID mismatch"
                    );
                    drop(socket);
                    continue;
                }

                tracing::debug!(client_id, peer_uid = ?peer_uid, "accepted IPC client connection");

                let (out_tx, out_rx) = mpsc::channel::<Arc<str>>(max_client_queue);
                let cancel = CancellationToken::new();
                clients.lock().await.insert(
                    client_id,
                    ClientHandle {
                        tx: out_tx.clone(),
                        cancel: cancel.clone(),
                    },
                );

                let clients_for_remove = clients.clone();
                let cmd_tx_for_client = cmd_tx.clone();
                let disconnected_for_client = disconnected_tx.clone();

                tasks.spawn(async move {
                    let _ = handle_client(
                        socket,
                        client_id,
                        out_tx,
                        out_rx,
                        cmd_tx_for_client,
                        cancel.clone(),
                    )
                    .await;
                    if let Some(client) = clients_for_remove.lock().await.remove(&client_id) {
                        client.cancel.cancel();
                        let _ = disconnected_for_client.send(client_id);
                    }
                });
            }
        });
    }
}

/// Bounded exponential backoff for repeated accept failures (EMFILE,
/// resource exhaustion). Capped at one second so recovery stays prompt
/// while a persistent error can no longer spin the accept loop.
fn accept_backoff(failures: u32) -> Duration {
    Duration::from_millis((10u64 << failures.min(6)).min(1_000))
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
