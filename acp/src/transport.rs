use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};

use crate::crypto;
use crate::discovery::{PeerInfo, PeerTable};
use crate::protocol::Envelope;

const MAX_MSG_SIZE: u32 = 65536;

/// Active peer connections keyed by agent_id.
pub type ConnectionTable = Arc<RwLock<HashMap<String, mpsc::Sender<Vec<u8>>>>>;

pub fn new_connection_table() -> ConnectionTable {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Channel for inbound envelopes (delivered to socket/IPC layer).
pub type InboundTx = mpsc::Sender<Envelope>;
pub type InboundRx = mpsc::Receiver<Envelope>;

pub fn inbound_channel() -> (InboundTx, InboundRx) {
    mpsc::channel(256)
}

/// Start the TCP listener.
pub async fn listen(
    port: u16,
    identity: Arc<crate::crypto::Identity>,
    peers: PeerTable,
    inbound_tx: InboundTx,
    connections: ConnectionTable,
) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(%port, "TCP listener started");

    loop {
        let (stream, addr) = listener.accept().await?;
        tracing::info!(%addr, "Accepted TCP connection");
        let identity = identity.clone();
        let peers = peers.clone();
        let inbound_tx = inbound_tx.clone();
        let connections = connections.clone();

        tokio::spawn(async move {
            if let Err(e) =
                handle_connection(stream, identity, peers, inbound_tx, connections).await
            {
                tracing::warn!(%addr, "Connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    identity: Arc<crypto::Identity>,
    peers: PeerTable,
    inbound_tx: InboundTx,
    connections: ConnectionTable,
) -> Result<()> {
    // Read first message to identify peer
    loop {
        let data = read_frame(&mut stream).await?;
        if data.is_empty() {
            break;
        }

        // Try to decrypt with each known peer's derived key
        let table = peers.read().await;
        let mut decoded = None;
        for (id, peer) in table.iter() {
            let sym_key = crypto::derive_key(&identity.secret, &peer.public_key);
            // Try to parse AAD - we need to try decrypting to find which peer
            // Since AAD is part of the envelope, we try with empty AAD first for identification
            // Actually, we need to try all peers since we don't know who sent it yet
            if let Ok(plaintext) = crypto::decrypt(&sym_key, &data, b"") {
                if let Ok(envelope) = serde_json::from_slice::<Envelope>(&plaintext) {
                    decoded = Some((id.clone(), envelope));
                    break;
                }
            }
            // Also try with proper AAD by attempting to parse the nonce+ciphertext
            // We'll use empty AAD for simplicity in v0.1
        }
        drop(table);

        if let Some((_peer_id, envelope)) = decoded {
            tracing::debug!(from = %envelope.from, kind = ?envelope.kind, "Received message");
            let _ = inbound_tx.send(envelope).await;
        } else {
            tracing::warn!("Could not decrypt message from any known peer");
        }
    }
    Ok(())
}

/// Read a length-prefixed frame from a TCP stream.
async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let len = match stream.read_u32().await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    if len > MAX_MSG_SIZE {
        anyhow::bail!("message too large: {len}");
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Write a length-prefixed frame to a TCP stream.
async fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    let len = data.len() as u32;
    stream.write_u32(len).await?;
    stream.write_all(data).await?;
    stream.flush().await?;
    Ok(())
}

/// Send an envelope to a specific peer.
pub async fn send_to_peer(
    envelope: &Envelope,
    identity: &crypto::Identity,
    peers: &PeerTable,
    connections: &ConnectionTable,
) -> Result<()> {
    let table = peers.read().await;
    let peer = table
        .get(&envelope.to)
        .ok_or_else(|| anyhow::anyhow!("unknown peer: {}", envelope.to))?
        .clone();
    drop(table);

    let sym_key = crypto::derive_key(&identity.secret, &peer.public_key);
    let plaintext = serde_json::to_vec(envelope)?;
    let encrypted = crypto::encrypt(&sym_key, &plaintext, b"")?;

    // Try to reuse existing connection, otherwise connect
    let mut conns = connections.write().await;
    if let Some(tx) = conns.get(&envelope.to) {
        if tx.send(encrypted.clone()).await.is_ok() {
            return Ok(());
        }
        // Channel closed, remove and reconnect
        conns.remove(&envelope.to);
    }

    // Open new connection
    let addr = format!("{}:{}", peer.addr, peer.port);
    let mut stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connect to {addr}"))?;
    tracing::info!(%addr, agent_id = %peer.agent_id, "Connected to peer");

    // Send this message directly
    write_frame(&mut stream, &encrypted).await?;

    // Set up a writer task for future messages
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    conns.insert(envelope.to.clone(), tx);
    drop(conns);

    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if write_frame(&mut stream, &data).await.is_err() {
                break;
            }
        }
    });

    Ok(())
}

/// Proactively connect to peers where we have the lower agent_id.
pub async fn connect_to_peers(
    my_agent_id: String,
    identity: Arc<crypto::Identity>,
    peers: PeerTable,
    inbound_tx: InboundTx,
    connections: ConnectionTable,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let table = peers.read().await;
        let peer_list: Vec<PeerInfo> = table.values().cloned().collect();
        drop(table);

        for peer in peer_list {
            // Lower agent_id initiates
            if my_agent_id >= peer.agent_id {
                continue;
            }

            let conns = connections.read().await;
            if conns.contains_key(&peer.agent_id) {
                continue;
            }
            drop(conns);

            let addr = format!("{}:{}", peer.addr, peer.port);
            match TcpStream::connect(&addr).await {
                Ok(stream) => {
                    tracing::info!(peer = %peer.agent_id, "Proactive connection established");
                    let identity = identity.clone();
                    let peers = peers.clone();
                    let inbound_tx = inbound_tx.clone();
                    let connections = connections.clone();
                    let peer_id = peer.agent_id.clone();

                    // Set up writer channel
                    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
                    {
                        let mut conns = connections.write().await;
                        conns.insert(peer_id.clone(), tx);
                    }

                    tokio::spawn(async move {
                        let (mut reader, mut writer) = stream.into_split();

                        // Writer task
                        let write_handle = tokio::spawn(async move {
                            while let Some(data) = rx.recv().await {
                                let len = data.len() as u32;
                                if writer.write_u32(len).await.is_err() {
                                    break;
                                }
                                if writer.write_all(&data).await.is_err() {
                                    break;
                                }
                                let _ = writer.flush().await;
                            }
                        });

                        // Reader
                        loop {
                            let len = match reader.read_u32().await {
                                Ok(l) => l,
                                Err(_) => break,
                            };
                            if len > MAX_MSG_SIZE {
                                break;
                            }
                            let mut buf = vec![0u8; len as usize];
                            if reader.read_exact(&mut buf).await.is_err() {
                                break;
                            }

                            let table = peers.read().await;
                            if let Some(p) = table.get(&peer_id) {
                                let sym_key =
                                    crypto::derive_key(&identity.secret, &p.public_key);
                                if let Ok(plaintext) = crypto::decrypt(&sym_key, &buf, b"") {
                                    if let Ok(envelope) =
                                        serde_json::from_slice::<Envelope>(&plaintext)
                                    {
                                        let _ = inbound_tx.send(envelope).await;
                                    }
                                }
                            }
                        }

                        write_handle.abort();
                        let mut conns = connections.write().await;
                        conns.remove(&peer_id);
                    });
                }
                Err(_) => {}
            }
        }
    }
}
