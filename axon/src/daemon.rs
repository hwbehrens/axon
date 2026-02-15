use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use crate::identity::Identity;
use crate::peer::{PeerTable, PeerStatus};
use crate::transport::{Transport, send_envelope, recv_envelope};
use crate::ipc::{IpcServer, IpcCommand, IpcResponse};
use crate::discovery::{MdnsDiscovery, StaticDiscovery, Discovery, PeerEvent};
use crate::message::{Envelope, HelloPayload};
use crate::config::Config;
use tracing::{info, warn, error};
use quinn::Connection;
use serde_json::json;
use std::collections::HashMap;

pub struct Daemon {
    identity: Identity,
    peer_table: Arc<PeerTable>,
    transport: Arc<Transport>,
    base_dir: PathBuf,
    connections: Arc<RwLock<HashMap<String, Connection>>>,
}

impl Daemon {
    pub async fn new(base_dir: PathBuf, port_override: Option<u16>) -> Result<Self> {
        let config = Config::load(&base_dir).await?;
        let port = port_override.unwrap_or(config.port);
        let identity = Identity::load_or_generate(&base_dir).await?;
        let peer_table = Arc::new(PeerTable::new());
        peer_table.load_known_peers(&base_dir).await?;
        
        let transport = Arc::new(Transport::new(&identity, port, peer_table.clone())?);

        Ok(Self {
            identity,
            peer_table,
            transport,
            base_dir,
            connections: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn run(self) -> Result<()> {
        info!("Starting AXON daemon (Agent ID: {})", self.identity.agent_id());

        let (peer_tx, mut peer_rx) = mpsc::channel(32);
        
        let config = Config::load(&self.base_dir).await?;
        let static_discovery = StaticDiscovery::new(config.peers);
        tokio::spawn(Box::new(static_discovery).run(peer_tx.clone()));

        let mdns_discovery = MdnsDiscovery::new(
            self.identity.agent_id(),
            self.identity.verifying_key().to_bytes().to_vec(),
            self.transport.endpoint.local_addr()?.port(),
        );
        tokio::spawn(Box::new(mdns_discovery).run(peer_tx));

        let ipc_path = self.base_dir.join(".axon/axon.sock");
        let ipc_server = IpcServer::new(&ipc_path)?;
        
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        
        #[cfg(unix)]
        {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            let tx = shutdown_tx.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = sigterm.recv() => info!("Received SIGTERM"),
                    _ = sigint.recv() => info!("Received SIGINT"),
                }
                let _ = tx.send(()).await;
            });
        }

        loop {
            tokio::select! {
                Some(event) = peer_rx.recv() => {
                    self.handle_peer_event(event).await;
                }
                Ok(ipc_stream) = ipc_server.accept() => {
                    let peer_table = self.peer_table.clone();
                    let connections = self.connections.clone();
                    let identity = Identity { signing_key: self.identity.signing_key.clone() };
                    tokio::spawn(async move {
                        if let Err(e) = handle_ipc_client(ipc_stream, peer_table, connections, identity).await {
                            error!("IPC client error: {}", e);
                        }
                    });
                }
                Some(connecting) = self.transport.accept() => {
                    let identity = Identity { signing_key: self.identity.signing_key.clone() };
                    let connections = self.connections.clone();
                    let peer_table = self.peer_table.clone();
                    tokio::spawn(async move {
                        if let Ok(conn) = connecting.await {
                            if let Err(e) = handle_connection(conn, identity, connections, peer_table).await {
                                warn!("Connection error: {}", e);
                            }
                        }
                    });
                }
                _ = shutdown_rx.recv() => {
                    info!("Shutting down...");
                    break;
                }
            }
        }

        self.peer_table.save_known_peers(&self.base_dir).await?;
        if ipc_path.exists() {
            let _ = std::fs::remove_file(ipc_path);
        }
        Ok(())
    }

    async fn handle_peer_event(&self, event: PeerEvent) {
        match event {
            PeerEvent::Discovered { agent_id, addr, pubkey } => {
                if agent_id == self.identity.agent_id() { return; }
                info!("Discovered peer {} at {}", agent_id, addr);
                self.peer_table.update_peer(agent_id.clone(), addr, pubkey).await;
                
                let conns = self.connections.read().await;
                if !conns.contains_key(&agent_id) && self.identity.agent_id() < agent_id {
                    drop(conns);
                    info!("Initiating connection to {}", agent_id);
                    let transport = self.transport.clone();
                    let identity = Identity { signing_key: self.identity.signing_key.clone() };
                    let connections = self.connections.clone();
                    let peer_table = self.peer_table.clone();
                    tokio::spawn(async move {
                        if let Ok(conn) = transport.connect(addr, &agent_id).await {
                            if let Err(e) = handle_connection(conn, identity, connections, peer_table).await {
                                warn!("Outbound connection error to {}: {}", agent_id, e);
                            }
                        }
                    });
                }
            }
            PeerEvent::Lost { agent_id } => {
                info!("Lost peer {}", agent_id);
                self.peer_table.set_status(&agent_id, PeerStatus::Disconnected).await;
            }
        }
    }
}

async fn handle_ipc_client(
    mut stream: tokio::net::UnixStream, 
    peer_table: Arc<PeerTable>,
    connections: Arc<RwLock<HashMap<String, Connection>>>,
    identity: Identity,
) -> Result<()> {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        let cmd: IpcCommand = serde_json::from_str(&line)?;
        let response = match cmd {
            IpcCommand::Peers => {
                let peers = peer_table.list_peers().await;
                IpcResponse {
                    ok: true,
                    msg_id: None,
                    peers: Some(json!(peers.into_iter().map(|p| json!({
                        "id": p.agent_id,
                        "addr": p.addr.to_string(),
                        "status": format!("{:?}", p.status),
                    })).collect::<Vec<_>>())),
                    error: None,
                    uptime_secs: None,
                }
            }
            IpcCommand::Status => {
                IpcResponse {
                    ok: true,
                    msg_id: None,
                    peers: None,
                    error: None,
                    uptime_secs: Some(0),
                }
            }
            IpcCommand::Send { to, kind, payload } => {
                let conns = connections.read().await;
                if let Some(conn) = conns.get(&to) {
                    let is_bidir = matches!(kind.as_str(), "query" | "delegate" | "discover" | "cancel" | "ping");
                    let envelope = Envelope::new(identity.agent_id(), to, kind, payload);
                    let msg_id = envelope.id.to_string();
                    
                    if is_bidir {
                        let (mut send, mut _recv) = conn.open_bi().await?;
                        send_envelope(&mut send, &envelope).await?;
                        // TODO: Wait for response if needed by IPC protocol
                    } else {
                        let mut send = conn.open_uni().await?;
                        send_envelope(&mut send, &envelope).await?;
                    }
                    
                    IpcResponse {
                        ok: true,
                        msg_id: Some(msg_id),
                        peers: None,
                        error: None,
                        uptime_secs: None,
                    }
                } else {
                    IpcResponse {
                        ok: false,
                        msg_id: None,
                        peers: None,
                        error: Some(format!("Peer not connected: {}", to)),
                        uptime_secs: None,
                    }
                }
            }
        };

        let resp_json = serde_json::to_string(&response)? + "\n";
        writer.write_all(resp_json.as_bytes()).await?;
        line.clear();
    }
    Ok(())
}

async fn handle_connection(
    conn: Connection, 
    identity: Identity,
    connections: Arc<RwLock<HashMap<String, Connection>>>,
    peer_table: Arc<PeerTable>,
) -> Result<()> {
    let (mut send, mut recv) = conn.open_bi().await?;
    
    let hello = Envelope::new(
        identity.agent_id(),
        "unknown".to_string(),
        "hello".to_string(),
        json!(HelloPayload {
            protocol_versions: vec![1],
            agent_name: None,
            features: vec!["ping".to_string(), "query".to_string()],
        })
    );

    send_envelope(&mut send, &hello).await?;
    let response = recv_envelope(&mut recv).await?;
    let remote_id = response.from.clone();
    info!("Handshake complete with {}", remote_id);

    {
        let mut conns = connections.write().await;
        conns.insert(remote_id.clone(), conn.clone());
    }
    peer_table.set_status(&remote_id, PeerStatus::Connected).await;

    loop {
        tokio::select! {
            result = conn.accept_uni() => {
                let mut recv = result?;
                let envelope = recv_envelope(&mut recv).await?;
                info!("Received uni message from {}: {:?}", remote_id, envelope);
            }
            result = conn.accept_bi() => {
                let (mut _send, mut recv) = result?;
                let envelope = recv_envelope(&mut recv).await?;
                info!("Received bi message from {}: {:?}", remote_id, envelope);
            }
            _ = conn.closed() => {
                info!("Connection closed by {}", remote_id);
                break;
            }
        }
    }

    {
        let mut conns = connections.write().await;
        conns.remove(&remote_id);
    }
    peer_table.set_status(&remote_id, PeerStatus::Disconnected).await;
    Ok(())
}
