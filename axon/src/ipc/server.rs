use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::auth;
use super::protocol::{CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, WhoamiInfo};
use crate::message::Envelope;

const MAX_IPC_LINE_LENGTH: usize = 64 * 1024; // 64 KB, aligned with MAX_MESSAGE_SIZE

static INVALID_COMMAND_LINE: LazyLock<Arc<str>> = LazyLock::new(|| {
    let error = super::protocol::IpcErrorCode::InvalidCommand;
    Arc::from(
        serde_json::to_string(&DaemonReply::Error {
            ok: false,
            message: error.message(),
            error,
            req_id: None,
        })
        .expect("static error serialization"),
    )
});

static COMMAND_TOO_LARGE_LINE: LazyLock<Arc<str>> = LazyLock::new(|| {
    let error = super::protocol::IpcErrorCode::CommandTooLarge;
    Arc::from(
        serde_json::to_string(&DaemonReply::Error {
            ok: false,
            message: error.message(),
            error,
            req_id: None,
        })
        .expect("static error serialization"),
    )
});

#[derive(Clone)]
struct ClientHandle {
    tx: mpsc::Sender<Arc<str>>,
    cancel: CancellationToken,
}

// ---------------------------------------------------------------------------
// IPC server config
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// IPC server
// ---------------------------------------------------------------------------

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

        if let Some(parent) = socket_path.parent()
            && !parent.exists()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create socket dir: {}", parent.display()))?;
        }

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind unix socket: {}", socket_path.display()))?;
        tokio::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
            .await
            .with_context(|| {
                format!(
                    "failed to set socket permissions: {}",
                    socket_path.display()
                )
            })?;

        let owner_uid = unsafe { libc::getuid() };
        let max_client_queue = config.max_client_queue;

        let server = Self {
            socket_path,
            max_clients,
            clients: Arc::new(Mutex::new(HashMap::new())),
            next_client_id: Arc::new(AtomicU64::new(1)),
            owner_uid,
            max_client_queue,
            config: Arc::new(config),
        };

        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        server.start_accept_loop(listener, cmd_tx);

        Ok((server, cmd_rx))
    }

    pub async fn send_reply(&self, client_id: u64, reply: &DaemonReply) -> Result<()> {
        let line: Arc<str> =
            Arc::from(serde_json::to_string(reply).context("failed to serialize daemon reply")?);
        let tx = {
            let clients = self.clients.lock().await;
            clients.get(&client_id).map(|client| client.tx.clone())
        };
        if let Some(tx) = tx {
            let send_result =
                tokio::time::timeout(std::time::Duration::from_secs(2), tx.send(line)).await;
            match send_result {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => {
                    self.close_client(client_id).await;
                }
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
        let line: Arc<str> = Arc::from(serde_json::to_string(&event)?);
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
            }
        }
        Ok(())
    }

    pub async fn handle_command(&self, event: CommandEvent) -> Result<DaemonReply> {
        match event.command {
            IpcCommand::Whoami { req_id } => Ok(DaemonReply::Whoami {
                ok: true,
                info: WhoamiInfo {
                    agent_id: self.config.agent_id.clone(),
                    public_key: self.config.public_key.clone(),
                    name: self.config.name.clone(),
                    version: self.config.version.clone(),
                    uptime_secs: (self.config.uptime_secs)(),
                },
                req_id,
            }),
            _ => Ok(DaemonReply::Error {
                ok: false,
                error: IpcErrorCode::InternalError,
                message: IpcErrorCode::InternalError.message(),
                req_id: event.command.req_id().map(|s| s.to_string()),
            }),
        }
    }

    /// Close a client connection by removing it from the client map and
    /// signaling cancellation to terminate its read/write loops.
    pub async fn close_client(&self, client_id: u64) {
        if let Some(client) = self.clients.lock().await.remove(&client_id) {
            client.cancel.cancel();
        }
    }

    pub async fn client_count(&self) -> usize {
        self.clients.lock().await.len()
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

    fn start_accept_loop(&self, listener: UnixListener, cmd_tx: mpsc::Sender<CommandEvent>) {
        let clients = self.clients.clone();
        let next_client_id = self.next_client_id.clone();
        let max_clients = self.max_clients;
        let owner_uid = self.owner_uid;
        let max_client_queue = self.max_client_queue;

        tokio::spawn(async move {
            loop {
                let (socket, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to accept IPC connection");
                        continue;
                    }
                };

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

                tokio::spawn(async move {
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
                    }
                });
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Per-client connection handler
// ---------------------------------------------------------------------------

async fn handle_client(
    socket: UnixStream,
    client_id: u64,
    out_tx: mpsc::Sender<Arc<str>>,
    mut out_rx: mpsc::Receiver<Arc<str>>,
    cmd_tx: mpsc::Sender<CommandEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    #[derive(Clone, Copy)]
    enum WriterCloseMode {
        Immediate,
        FlushQueued,
    }

    let (read_half, mut write_half) = socket.into_split();
    let writer_cancel = cancel.clone();
    let (writer_close_tx, mut writer_close_rx) = oneshot::channel::<WriterCloseMode>();

    let mut writer_handle = tokio::spawn(async move {
        let mut close_mode = WriterCloseMode::Immediate;
        loop {
            tokio::select! {
                _ = writer_cancel.cancelled() => break,
                mode = &mut writer_close_rx => {
                    close_mode = mode.unwrap_or(WriterCloseMode::Immediate);
                    break;
                }
                maybe_line = out_rx.recv() => {
                    let Some(line) = maybe_line else {
                        break;
                    };
                    if write_half.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if write_half.write_all(b"\n").await.is_err() {
                        break;
                    }
                }
            }
        }

        if matches!(close_mode, WriterCloseMode::FlushQueued) {
            while let Ok(line) = out_rx.try_recv() {
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if write_half.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        }

        let _ = write_half.shutdown().await;
    });

    let mut reader = BufReader::new(read_half);
    let mut buf = Vec::with_capacity(MAX_IPC_LINE_LENGTH + 1);
    let mut writer_close_mode = WriterCloseMode::Immediate;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        buf.clear();
        let mut found_newline = false;
        let mut exceeded = false;

        loop {
            let available = tokio::select! {
                _ = cancel.cancelled() => break,
                read_result = reader.fill_buf() => read_result.context("failed reading IPC")?,
            };
            if available.is_empty() {
                break; // EOF
            }

            if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                let needed = pos;
                if buf.len() + needed > MAX_IPC_LINE_LENGTH {
                    exceeded = true;
                    reader.consume(pos + 1);
                    break;
                }
                buf.extend_from_slice(&available[..pos]);
                reader.consume(pos + 1);
                found_newline = true;
                break;
            } else {
                let len = available.len();
                if buf.len() + len > MAX_IPC_LINE_LENGTH {
                    exceeded = true;
                    reader.consume(len);
                    break;
                }
                buf.extend_from_slice(available);
                reader.consume(len);
            }
        }

        if exceeded {
            let _ = out_tx.send(COMMAND_TOO_LARGE_LINE.clone()).await;
            writer_close_mode = WriterCloseMode::FlushQueued;
            break; // Close connection — can't reliably find next command boundary
        }

        if !found_newline {
            break; // EOF
        }
        let line = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => {
                let _ = out_tx.try_send(INVALID_COMMAND_LINE.clone());
                continue;
            }
        };
        match serde_json::from_str::<IpcCommand>(line) {
            Ok(command) => {
                cmd_tx
                    .send(CommandEvent { client_id, command })
                    .await
                    .map_err(|_| anyhow::anyhow!("daemon command channel closed"))?;
            }
            Err(_err) => {
                let _ = out_tx.try_send(INVALID_COMMAND_LINE.clone());
            }
        }
    }

    let _ = writer_close_tx.send(writer_close_mode);
    if tokio::time::timeout(std::time::Duration::from_secs(1), &mut writer_handle)
        .await
        .is_err()
    {
        writer_handle.abort();
        tracing::warn!(
            client_id,
            "timed out waiting for IPC client writer shutdown"
        );
    }

    cancel.cancel();
    Ok(())
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
