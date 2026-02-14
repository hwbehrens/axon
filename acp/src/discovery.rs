use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use x25519_dalek::PublicKey;

use crate::crypto;

const SERVICE_TYPE: &str = "_acp._tcp.local.";
const STALE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub agent_id: String,
    pub addr: std::net::IpAddr,
    pub port: u16,
    pub public_key: PublicKey,
    pub last_seen: Instant,
}

pub type PeerTable = Arc<RwLock<HashMap<String, PeerInfo>>>;

pub fn new_peer_table() -> PeerTable {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Register our service via mDNS.
pub fn advertise(
    mdns: &ServiceDaemon,
    agent_id: &str,
    port: u16,
    pubkey_b64: &str,
) -> Result<()> {
    let props = [
        ("agent_id", agent_id),
        ("pubkey", pubkey_b64),
    ];
    let host = format!("{}.local.", agent_id);
    let service = ServiceInfo::new(
        SERVICE_TYPE,
        agent_id,
        &host,
        "",
        port,
        &props[..],
    )
    .map_err(|e| anyhow::anyhow!("ServiceInfo: {e}"))?
    .enable_addr_auto();

    mdns.register(service)
        .map_err(|e| anyhow::anyhow!("mDNS register: {e}"))?;
    tracing::info!(%agent_id, %port, "Advertised on mDNS");
    Ok(())
}

/// Background task: browse for peers and update the peer table.
pub async fn browse(
    mdns: ServiceDaemon,
    peers: PeerTable,
    my_agent_id: String,
) -> Result<()> {
    let receiver = mdns
        .browse(SERVICE_TYPE)
        .map_err(|e| anyhow::anyhow!("mDNS browse: {e}"))?;

    // Spawn a blocking task since mdns-sd receiver is std sync
    tokio::task::spawn_blocking(move || {
        while let Ok(event) = receiver.recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let Some(agent_id) = info.get_property_val_str("agent_id") else {
                        continue;
                    };
                    if agent_id == my_agent_id {
                        continue; // skip self
                    }
                    let Some(pubkey_b64) = info.get_property_val_str("pubkey") else {
                        continue;
                    };
                    let Ok(public_key) = crypto::parse_public_key(pubkey_b64) else {
                        tracing::warn!(%agent_id, "bad pubkey from mDNS");
                        continue;
                    };
                    let addr = info.get_addresses().iter().next().copied();
                    let Some(addr) = addr else { continue };
                    let port = info.get_port();

                    tracing::info!(%agent_id, %addr, %port, "Discovered peer");

                    let peer = PeerInfo {
                        agent_id: agent_id.to_string(),
                        addr: addr,
                        port,
                        public_key,
                        last_seen: Instant::now(),
                    };

                    // Block on acquiring write lock (we're in blocking task)
                    let peers_clone = peers.clone();
                    let id = agent_id.to_string();
                    // Use a std mutex pattern via try_write or just spawn a tiny tokio task
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async {
                        let mut table = peers_clone.write().await;
                        table.insert(id, peer);
                    });
                }
                ServiceEvent::ServiceRemoved(_type, fullname) => {
                    let rt = tokio::runtime::Handle::current();
                    let peers_clone = peers.clone();
                    rt.block_on(async {
                        let mut table = peers_clone.write().await;
                        // fullname format varies; try to match by agent_id
                        table.retain(|_, p| {
                            // Keep if last_seen is recent enough or name doesn't match
                            p.last_seen.elapsed() < STALE_TIMEOUT
                        });
                    });
                }
                _ => {}
            }
        }
    });

    Ok(())
}

/// Periodic cleanup of stale peers.
pub async fn cleanup_loop(peers: PeerTable) {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let mut table = peers.write().await;
        table.retain(|id, p| {
            let stale = p.last_seen.elapsed() > STALE_TIMEOUT;
            if stale {
                tracing::info!(%id, "Removing stale peer");
            }
            !stale
        });
    }
}
