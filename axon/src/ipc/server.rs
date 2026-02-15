use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, mpsc};

use super::auth;
use super::backend::IpcBackend;
use super::handlers::{ClientState, DispatchResult, IpcHandlers};
use super::protocol::{CommandEvent, DaemonReply, IpcCommand};
use super::receive_buffer::ReceiveBuffer;
use crate::message::Envelope;

const MAX_CLIENT_QUEUE: usize = 1024;
const MAX_IPC_LINE_LENGTH: usize = 256 * 1024; // 256 KB

// Re-export config and types for public API
pub use super::handlers::IpcServerConfig;

// ---------------------------------------------------------------------------
// IPC server
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct IpcServer {
    socket_path: PathBuf,
    max_clients: usize,
    clients: Arc<Mutex<HashMap<u64, mpsc::Sender<Arc<str>>>>>,
    client_states: Arc<Mutex<HashMap<u64, ClientState>>>,
    next_client_id: Arc<AtomicU64>,
    handlers: Arc<IpcHandlers>,
    owner_uid: u32,
}

impl IpcServer {
    pub async fn bind(
        socket_path: PathBuf,
        max_clients: usize,
        config: IpcServerConfig,
    ) -> Result<(Self, mpsc::Receiver<CommandEvent>)> {
        if socket_path.exists() {
            let meta = fs::symlink_metadata(&socket_path).with_context(|| {
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
            fs::remove_file(&socket_path).with_context(|| {
                format!(
                    "failed to remove stale unix socket: {}",
                    socket_path.display()
                )
            })?;
        }

        if let Some(parent) = socket_path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create socket dir: {}", parent.display()))?;
        }

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind unix socket: {}", socket_path.display()))?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).with_context(
            || {
                format!(
                    "failed to set socket permissions: {}",
                    socket_path.display()
                )
            },
        )?;

        let owner_uid = unsafe { libc::getuid() };

        let clients = Arc::new(Mutex::new(HashMap::new()));
        let client_states = Arc::new(Mutex::new(HashMap::new()));
        let mut buffer = ReceiveBuffer::new(config.buffer_size, config.buffer_ttl_secs);
        if let Some(byte_cap) = config.buffer_byte_cap {
            buffer = buffer.with_byte_cap(byte_cap);
        }
        let receive_buffer = Arc::new(Mutex::new(buffer));

        let handlers = Arc::new(IpcHandlers::new(
            Arc::new(config),
            client_states.clone(),
            receive_buffer,
            clients.clone(),
        ));

        let server = Self {
            socket_path,
            max_clients,
            clients,
            client_states,
            next_client_id: Arc::new(AtomicU64::new(1)),
            handlers,
            owner_uid,
        };

        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        server.start_accept_loop(listener, cmd_tx);

        Ok((server, cmd_rx))
    }

    pub async fn send_reply(&self, client_id: u64, reply: &DaemonReply) -> Result<()> {
        let line: Arc<str> =
            Arc::from(serde_json::to_string(reply).context("failed to serialize daemon reply")?);
        if let Some(tx) = self.clients.lock().await.get(&client_id).cloned() {
            let _ = tx.try_send(line);
        }
        Ok(())
    }

    pub async fn broadcast_inbound(&self, envelope: &Envelope) -> Result<()> {
        self.handlers.broadcast_inbound(envelope).await
    }

    pub async fn handle_command(&self, event: CommandEvent) -> Result<DaemonReply> {
        self.handlers
            .handle_command(event.client_id, event.command)
            .await
    }

    /// Dispatch a command through unified IPC policy enforcement, delegating
    /// Send/Peers/Status effects to the provided backend.
    pub async fn dispatch_command(
        &self,
        client_id: u64,
        command: IpcCommand,
        backend: &(impl IpcBackend + ?Sized),
    ) -> Result<DispatchResult> {
        self.handlers
            .dispatch_command(client_id, command, backend)
            .await
    }

    /// Close a client connection by removing it from the client map.
    /// The client's write loop will end when the sender is dropped.
    pub async fn close_client(&self, client_id: u64) {
        self.clients.lock().await.remove(&client_id);
        self.client_states.lock().await.remove(&client_id);
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
        let client_states = self.client_states.clone();
        let next_client_id = self.next_client_id.clone();
        let max_clients = self.max_clients;
        let owner_uid = self.owner_uid;

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

                // Check peer credentials for implicit authentication
                let peer_authenticated = if let Some(peer_uid) = auth::peer_uid(&socket) {
                    peer_uid == owner_uid
                } else {
                    false
                };

                let (out_tx, out_rx) = mpsc::channel::<Arc<str>>(MAX_CLIENT_QUEUE);
                clients.lock().await.insert(client_id, out_tx.clone());

                let state = ClientState {
                    authenticated: peer_authenticated,
                    ..Default::default()
                };
                client_states.lock().await.insert(client_id, state);

                let clients_for_remove = clients.clone();
                let client_states_for_remove = client_states.clone();
                let cmd_tx_for_client = cmd_tx.clone();

                tokio::spawn(async move {
                    let _ =
                        handle_client(socket, client_id, out_tx, out_rx, cmd_tx_for_client).await;
                    clients_for_remove.lock().await.remove(&client_id);
                    client_states_for_remove.lock().await.remove(&client_id);
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
) -> Result<()> {
    let (read_half, mut write_half) = socket.into_split();

    tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if write_half.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if write_half.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await.context("failed reading IPC line")? {
        if line.len() > MAX_IPC_LINE_LENGTH {
            let err_line = serde_json::to_string(&DaemonReply::Error {
                ok: false,
                error: super::protocol::IpcErrorCode::InvalidCommand,
                req_id: None,
            })
            .context("failed to serialize IPC length error")?;
            let _ = out_tx.try_send(Arc::from(err_line));
            continue;
        }
        match serde_json::from_str::<IpcCommand>(&line) {
            Ok(command) => {
                cmd_tx
                    .send(CommandEvent { client_id, command })
                    .await
                    .map_err(|_| anyhow::anyhow!("daemon command channel closed"))?;
            }
            Err(_err) => {
                let err_line = serde_json::to_string(&DaemonReply::Error {
                    ok: false,
                    error: super::protocol::IpcErrorCode::InvalidCommand,
                    req_id: None,
                })
                .context("failed to serialize IPC parse error")?;
                let _ = out_tx.try_send(Arc::from(err_line));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "server_tests/mod.rs"]
mod tests;
