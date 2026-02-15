use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use crate::identity::Identity;
use crate::peer::PeerTable;
use crate::transport::{Transport, send_envelope, recv_envelope};
use crate::ipc::{IpcServer, IpcCommand, IpcResponse};
use crate::discovery::{MdnsDiscovery, Discovery, PeerEvent};
use crate::message::{Envelope, HelloPayload};
use tracing::{info, error};
use quinn::Connection;
use serde_json::json;

pub struct Daemon {
    identity: Identity,
    peer_table: Arc<PeerTable>,
    transport: Arc<Transport>,
    base_dir: PathBuf,
}

impl Daemon {
    pub async fn new(base_dir: PathBuf, port: u16) -> Result<Self> {
        let identity = Identity::load_or_generate(&base_dir).await?;
        let peer_table = Arc::new(PeerTable::new());
        let transport = Arc::new(Transport::new(&identity, port)?);

        Ok(Self {
            identity,
            peer_table,
            transport,
            base_dir,
        })
    }

    pub async fn run(self) -> Result<()> {
        info!("Starting AXON daemon (Agent ID: {})", self.identity.agent_id());

        let (peer_tx, mut peer_rx) = mpsc::channel(32);
        let discovery = MdnsDiscovery::new(
            self.identity.agent_id(),
            self.identity.verifying_key().to_bytes().to_vec(),
            7100,
        );

        tokio::spawn(Box::new(discovery).run(peer_tx));

        let ipc_path = self.base_dir.join(".axon/axon.sock");
        let ipc_server = IpcServer::new(&ipc_path)?;
        
        loop {
            tokio::select! {
                Some(event) = peer_rx.recv() => {
                    self.handle_peer_event(event).await;
                }
                Ok(ipc_stream) = ipc_server.accept() => {
                    let peer_table = self.peer_table.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_ipc_client(ipc_stream, peer_table).await {
                            error!("IPC client error: {}", e);
                        }
                    });
                }
                Some(connecting) = self.transport.accept() => {
                    let identity = Identity { signing_key: self.identity.signing_key.clone() };
                    tokio::spawn(async move {
                        if let Ok(conn) = connecting.await {
                            let _ = handle_connection(conn, identity).await;
                        }
                    });
                }
            }
        }
    }

    async fn handle_peer_event(&self, event: PeerEvent) {
        match event {
            PeerEvent::Discovered { agent_id, addr, pubkey } => {
                info!("Discovered peer {} at {}", agent_id, addr);
                self.peer_table.update_peer(agent_id.clone(), addr, pubkey).await;
                
                if self.identity.agent_id() < agent_id {
                    info!("Initiating connection to {}", agent_id);
                    let transport = self.transport.clone();
                    let identity = Identity { signing_key: self.identity.signing_key.clone() };
                    tokio::spawn(async move {
                        if let Ok(conn) = transport.connect(addr).await {
                            let _ = handle_connection(conn, identity).await;
                        }
                    });
                }
            }
            PeerEvent::Lost { agent_id } => {
                info!("Lost peer {}", agent_id);
            }
        }
    }
}

async fn handle_ipc_client(mut stream: tokio::net::UnixStream, peer_table: Arc<PeerTable>) -> Result<()> {
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
                info!("IPC send to {}: {} {:?}", to, kind, payload);
                IpcResponse {
                    ok: true,
                    msg_id: Some(uuid::Uuid::new_v4().to_string()),
                    peers: None,
                    error: None,
                    uptime_secs: None,
                }
            }
        };

        let resp_json = serde_json::to_string(&response)? + "\n";
        writer.write_all(resp_json.as_bytes()).await?;
        line.clear();
    }
    Ok(())
}

async fn handle_connection(conn: Connection, identity: Identity) -> Result<()> {
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
    info!("Handshake complete with {}", response.from);

    loop {
        tokio::select! {
            result = conn.accept_uni() => {
                let mut recv = result?;
                let envelope = recv_envelope(&mut recv).await?;
                info!("Received uni message: {:?}", envelope);
            }
            result = conn.accept_bi() => {
                let (mut _send, mut recv) = result?;
                let envelope = recv_envelope(&mut recv).await?;
                info!("Received bi message: {:?}", envelope);
            }
        }
    }
}
