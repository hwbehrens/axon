use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use crate::message::{Envelope, MessageKind};

// ---------------------------------------------------------------------------
// IPC protocol types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum IpcCommand {
    Send {
        to: String,
        kind: MessageKind,
        payload: Value,
        #[serde(default, rename = "ref")]
        ref_id: Option<Uuid>,
    },
    Peers,
    Status,
}

#[derive(Debug, Clone)]
pub struct CommandEvent {
    pub client_id: u64,
    pub command: IpcCommand,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerSummary {
    pub id: String,
    pub addr: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DaemonReply {
    SendAck {
        ok: bool,
        msg_id: Uuid,
    },
    Peers {
        ok: bool,
        peers: Vec<PeerSummary>,
    },
    Status {
        ok: bool,
        uptime_secs: u64,
        peers_connected: usize,
        messages_sent: u64,
        messages_received: u64,
    },
    Error {
        ok: bool,
        error: String,
    },
    Inbound {
        inbound: bool,
        envelope: Envelope,
    },
}

// ---------------------------------------------------------------------------
// IPC server
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct IpcServer {
    socket_path: PathBuf,
    clients: Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>>,
    next_client_id: Arc<AtomicU64>,
}

impl IpcServer {
    pub async fn bind(socket_path: PathBuf) -> Result<(Self, mpsc::Receiver<CommandEvent>)> {
        if socket_path.exists() {
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

        let server = Self {
            socket_path,
            clients: Arc::new(Mutex::new(HashMap::new())),
            next_client_id: Arc::new(AtomicU64::new(1)),
        };

        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        server.start_accept_loop(listener, cmd_tx);

        Ok((server, cmd_rx))
    }

    pub async fn send_reply(&self, client_id: u64, reply: &DaemonReply) -> Result<()> {
        let line = serde_json::to_string(reply).context("failed to serialize daemon reply")?;
        if let Some(tx) = self.clients.lock().await.get(&client_id).cloned() {
            let _ = tx.send(line);
        }
        Ok(())
    }

    pub async fn broadcast_inbound(&self, envelope: Envelope) -> Result<()> {
        let msg = DaemonReply::Inbound {
            inbound: true,
            envelope,
        };
        let line = serde_json::to_string(&msg).context("failed to serialize inbound message")?;
        for tx in self.clients.lock().await.values() {
            let _ = tx.send(line.clone());
        }
        Ok(())
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

        tokio::spawn(async move {
            loop {
                let (socket, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let client_id = next_client_id.fetch_add(1, Ordering::Relaxed);
                let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
                clients.lock().await.insert(client_id, out_tx.clone());

                let clients_for_remove = clients.clone();
                let cmd_tx_for_client = cmd_tx.clone();

                tokio::spawn(async move {
                    let _ =
                        handle_client(socket, client_id, out_tx, out_rx, cmd_tx_for_client).await;
                    clients_for_remove.lock().await.remove(&client_id);
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
    out_tx: mpsc::UnboundedSender<String>,
    mut out_rx: mpsc::UnboundedReceiver<String>,
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
        match serde_json::from_str::<IpcCommand>(&line) {
            Ok(command) => {
                cmd_tx
                    .send(CommandEvent { client_id, command })
                    .await
                    .map_err(|_| anyhow::anyhow!("daemon command channel closed"))?;
            }
            Err(err) => {
                let err_line = serde_json::to_string(&DaemonReply::Error {
                    ok: false,
                    error: format!("invalid command: {err}"),
                })
                .context("failed to serialize IPC parse error")?;
                let _ = out_tx.send(err_line);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    // --- IpcCommand deserialization ---

    #[test]
    fn parse_send_command() {
        let parsed: IpcCommand = serde_json::from_value(json!({
            "cmd": "send",
            "to": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "kind": "notify",
            "payload": {"topic":"meta.status", "data":{}}
        }))
        .expect("parse command");

        match parsed {
            IpcCommand::Send { to, kind, .. } => {
                assert_eq!(to, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
                assert_eq!(kind, MessageKind::Notify);
            }
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn parse_send_with_ref() {
        let id = Uuid::new_v4();
        let parsed: IpcCommand = serde_json::from_value(json!({
            "cmd": "send",
            "to": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "kind": "cancel",
            "payload": {"reason": "changed mind"},
            "ref": id.to_string()
        }))
        .expect("parse command");

        match parsed {
            IpcCommand::Send { ref_id, .. } => {
                assert_eq!(ref_id, Some(id));
            }
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn parse_peers_command() {
        let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"peers"}"#).unwrap();
        assert!(matches!(cmd, IpcCommand::Peers));
    }

    #[test]
    fn parse_status_command() {
        let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
        assert!(matches!(cmd, IpcCommand::Status));
    }

    #[test]
    fn unknown_cmd_fails() {
        let result = serde_json::from_str::<IpcCommand>(r#"{"cmd":"explode"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_json_fails() {
        let result = serde_json::from_str::<IpcCommand>("not json");
        assert!(result.is_err());
    }

    // --- DaemonReply serialization ---

    #[test]
    fn send_ack_serialization() {
        let id = Uuid::new_v4();
        let reply = DaemonReply::SendAck { ok: true, msg_id: id };
        let json = serde_json::to_string(&reply).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["msg_id"], id.to_string());
    }

    #[test]
    fn peers_reply_serialization() {
        let reply = DaemonReply::Peers {
            ok: true,
            peers: vec![PeerSummary {
                id: "a1b2c3d4".to_string(),
                addr: "192.168.1.50:7100".to_string(),
                status: "connected".to_string(),
                rtt_ms: Some(0.4),
                source: "static".to_string(),
            }],
        };
        let json = serde_json::to_string(&reply).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["peers"][0]["id"], "a1b2c3d4");
        assert_eq!(v["peers"][0]["rtt_ms"], 0.4);
        assert_eq!(v["peers"][0]["source"], "static");
    }

    #[test]
    fn status_reply_serialization() {
        let reply = DaemonReply::Status {
            ok: true,
            uptime_secs: 3600,
            peers_connected: 2,
            messages_sent: 42,
            messages_received: 38,
        };
        let json = serde_json::to_string(&reply).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["uptime_secs"], 3600);
        assert_eq!(v["messages_sent"], 42);
    }

    #[test]
    fn error_reply_serialization() {
        let reply = DaemonReply::Error {
            ok: false,
            error: "peer not found: deadbeef".to_string(),
        };
        let json = serde_json::to_string(&reply).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("peer not found"));
    }

    #[test]
    fn inbound_reply_serialization() {
        let envelope = Envelope::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            MessageKind::Notify,
            json!({"topic":"meta.status", "data":{}}),
        );
        let reply = DaemonReply::Inbound {
            inbound: true,
            envelope,
        };
        let json = serde_json::to_string(&reply).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["inbound"], true);
        assert_eq!(v["envelope"]["kind"], "notify");
    }

    // --- IPC server integration ---

    #[tokio::test]
    async fn bind_creates_socket_file() {
        let dir = tempdir().expect("tempdir");
        let socket_path = dir.path().join("axon.sock");
        let (_server, _rx) = IpcServer::bind(socket_path.clone())
            .await
            .expect("bind IPC server");

        assert!(socket_path.exists());
    }

    #[tokio::test]
    async fn broadcasts_inbound_to_multiple_clients() {
        let dir = tempdir().expect("tempdir");
        let socket_path = dir.path().join("axon.sock");
        let (server, _rx) = IpcServer::bind(socket_path.clone())
            .await
            .expect("bind IPC server");

        let mut client_a = UnixStream::connect(&socket_path)
            .await
            .expect("connect client A");
        let mut client_b = UnixStream::connect(&socket_path)
            .await
            .expect("connect client B");

        // Give the accept loop a moment to register clients.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let envelope = Envelope::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            MessageKind::Notify,
            json!({"topic":"meta.status", "data":{}}),
        );
        server
            .broadcast_inbound(envelope)
            .await
            .expect("broadcast inbound");

        let mut line_a = String::new();
        let mut line_b = String::new();
        let mut reader_a = BufReader::new(&mut client_a);
        let mut reader_b = BufReader::new(&mut client_b);
        reader_a.read_line(&mut line_a).await.expect("read A");
        reader_b.read_line(&mut line_b).await.expect("read B");

        assert!(line_a.contains("\"inbound\":true"));
        assert!(line_b.contains("\"inbound\":true"));
    }

    #[tokio::test]
    async fn send_command_round_trip() {
        let dir = tempdir().expect("tempdir");
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone())
            .await
            .expect("bind IPC server");

        let mut client = UnixStream::connect(&socket_path)
            .await
            .expect("connect");

        client
            .write_all(b"{\"cmd\":\"peers\"}\n")
            .await
            .expect("write");

        let cmd = tokio::time::timeout(std::time::Duration::from_secs(2), cmd_rx.recv())
            .await
            .expect("timeout")
            .expect("recv");

        assert!(matches!(cmd.command, IpcCommand::Peers));

        server
            .send_reply(
                cmd.client_id,
                &DaemonReply::Peers {
                    ok: true,
                    peers: vec![],
                },
            )
            .await
            .expect("reply");

        let mut line = String::new();
        let mut reader = BufReader::new(&mut client);
        reader.read_line(&mut line).await.expect("read");
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["peers"], json!([]));
    }

    #[tokio::test]
    async fn invalid_command_returns_error() {
        let dir = tempdir().expect("tempdir");
        let socket_path = dir.path().join("axon.sock");
        let (_server, _rx) = IpcServer::bind(socket_path.clone())
            .await
            .expect("bind IPC server");

        let mut client = UnixStream::connect(&socket_path)
            .await
            .expect("connect");
        client
            .write_all(b"{\"cmd\":\"unknown\"}\n")
            .await
            .expect("write");

        let mut line = String::new();
        let mut reader = BufReader::new(client);
        reader.read_line(&mut line).await.expect("read");
        assert!(line.contains("\"ok\":false"));
        assert!(line.contains("invalid command"));
    }

    #[tokio::test]
    async fn cleanup_removes_socket() {
        let dir = tempdir().expect("tempdir");
        let socket_path = dir.path().join("axon.sock");
        let (server, _rx) = IpcServer::bind(socket_path.clone())
            .await
            .expect("bind IPC server");

        assert!(socket_path.exists());
        server.cleanup_socket().expect("cleanup");
        assert!(!socket_path.exists());
    }

    #[tokio::test]
    async fn client_disconnect_does_not_affect_others() {
        let dir = tempdir().expect("tempdir");
        let socket_path = dir.path().join("axon.sock");
        let (server, _rx) = IpcServer::bind(socket_path.clone())
            .await
            .expect("bind IPC server");

        let client_a = UnixStream::connect(&socket_path).await.expect("connect A");
        let mut client_b = UnixStream::connect(&socket_path).await.expect("connect B");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(server.client_count().await, 2);

        drop(client_a);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let envelope = Envelope::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            MessageKind::Ping,
            json!({}),
        );
        server.broadcast_inbound(envelope).await.expect("broadcast");

        let mut line = String::new();
        let mut reader = BufReader::new(&mut client_b);
        reader.read_line(&mut line).await.expect("read B");
        assert!(line.contains("\"inbound\":true"));
    }
}
