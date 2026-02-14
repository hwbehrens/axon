use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::RwLock;

use crate::crypto::Identity;
use crate::discovery::PeerTable;
use crate::protocol::Envelope;
use crate::transport::{self, ConnectionTable, InboundRx};

#[derive(Debug, Deserialize)]
struct Command {
    cmd: String,
    to: Option<String>,
    envelope: Option<serde_json::Value>,
    // For CLI convenience
    message: Option<String>,
    task: Option<String>,
    topic: Option<String>,
    data: Option<String>,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peers: Option<Vec<PeerStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uptime: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peers_connected: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct PeerStatus {
    id: String,
    addr: String,
    status: String,
}

pub struct DaemonState {
    pub identity: Arc<Identity>,
    pub agent_id: String,
    pub peers: PeerTable,
    pub connections: ConnectionTable,
    pub start_time: Instant,
    pub messages_sent: Arc<RwLock<u64>>,
}

pub fn socket_path(agent_id: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.acp/acp.sock", home)
}

/// Run the Unix domain socket listener.
pub async fn listen(state: Arc<DaemonState>, mut inbound_rx: InboundRx) -> Result<()> {
    let path = socket_path(&state.agent_id);

    // Remove stale socket
    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path)?;
    tracing::info!(%path, "Unix socket listening");

    // Track connected IPC clients for forwarding inbound messages
    let clients: Arc<RwLock<Vec<tokio::sync::mpsc::Sender<String>>>> =
        Arc::new(RwLock::new(Vec::new()));

    // Forward inbound network messages to IPC clients
    let clients_fwd = clients.clone();
    tokio::spawn(async move {
        while let Some(envelope) = inbound_rx.recv().await {
            let msg = serde_json::json!({
                "inbound": true,
                "envelope": envelope,
            });
            let line = serde_json::to_string(&msg).unwrap_or_default();
            let mut cl = clients_fwd.write().await;
            cl.retain(|tx| !tx.is_closed());
            for tx in cl.iter() {
                let _ = tx.send(line.clone()).await;
            }
        }
    });

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let clients = clients.clone();

        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();

            // Register this client for inbound forwarding
            let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
            {
                let mut cl = clients.write().await;
                cl.push(tx);
            }

            // Forward inbound messages to this client
            let writer2 = Arc::new(tokio::sync::Mutex::new(writer));
            let writer_fwd = writer2.clone();
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    let mut w = writer_fwd.lock().await;
                    let _ = w.write_all(line.as_bytes()).await;
                    let _ = w.write_all(b"\n").await;
                    let _ = w.flush().await;
                }
            });

            while let Ok(Some(line)) = lines.next_line().await {
                let response = handle_command(&line, &state).await;
                let resp_str = serde_json::to_string(&response).unwrap_or_default();
                let mut w = writer2.lock().await;
                let _ = w.write_all(resp_str.as_bytes()).await;
                let _ = w.write_all(b"\n").await;
                let _ = w.flush().await;
            }
        });
    }
}

async fn handle_command(line: &str, state: &DaemonState) -> Response {
    let cmd: Command = match serde_json::from_str(line) {
        Ok(c) => c,
        Err(e) => {
            return Response {
                ok: false,
                error: Some(format!("invalid JSON: {e}")),
                ..default_response()
            }
        }
    };

    match cmd.cmd.as_str() {
        "send" => {
            let to = match cmd.to {
                Some(t) => t,
                None => {
                    return Response {
                        ok: false,
                        error: Some("missing 'to' field".into()),
                        ..default_response()
                    }
                }
            };
            let envelope = if let Some(env_val) = cmd.envelope {
                match serde_json::from_value::<Envelope>(env_val) {
                    Ok(e) => e,
                    Err(e) => {
                        return Response {
                            ok: false,
                            error: Some(format!("invalid envelope: {e}")),
                            ..default_response()
                        }
                    }
                }
            } else if let Some(msg) = cmd.message {
                Envelope::new_query(&state.agent_id, &to, &msg)
            } else {
                return Response {
                    ok: false,
                    error: Some("need 'envelope' or 'message'".into()),
                    ..default_response()
                };
            };

            let msg_id = envelope.id.clone();
            match transport::send_to_peer(
                &envelope,
                &state.identity,
                &state.peers,
                &state.connections,
            )
            .await
            {
                Ok(()) => {
                    let mut sent = state.messages_sent.write().await;
                    *sent += 1;
                    Response {
                        ok: true,
                        msg_id: Some(msg_id),
                        ..default_response()
                    }
                }
                Err(e) => Response {
                    ok: false,
                    error: Some(format!("{e}")),
                    ..default_response()
                },
            }
        }
        "peers" => {
            let table = state.peers.read().await;
            let conns = state.connections.read().await;
            let peers: Vec<PeerStatus> = table
                .values()
                .map(|p| PeerStatus {
                    id: p.agent_id.clone(),
                    addr: format!("{}:{}", p.addr, p.port),
                    status: if conns.contains_key(&p.agent_id) {
                        "connected".into()
                    } else {
                        "discovered".into()
                    },
                })
                .collect();
            Response {
                ok: true,
                peers: Some(peers),
                ..default_response()
            }
        }
        "status" => {
            let uptime = state.start_time.elapsed().as_secs();
            let conns = state.connections.read().await;
            let sent = state.messages_sent.read().await;
            Response {
                ok: true,
                uptime: Some(uptime),
                peers_connected: Some(conns.len()),
                ..default_response()
            }
        }
        other => Response {
            ok: false,
            error: Some(format!("unknown command: {other}")),
            ..default_response()
        },
    }
}

fn default_response() -> Response {
    Response {
        ok: false,
        msg_id: None,
        peers: None,
        uptime: None,
        peers_connected: None,
        error: None,
    }
}
