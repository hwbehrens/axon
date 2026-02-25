use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::config::PersistedStaticPeerConfig;
use crate::message::AgentId;

pub const SERVICE_TYPE: &str = "_axon._udp.local.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    Discovered {
        agent_id: AgentId,
        addr: SocketAddr,
        pubkey: String,
    },
    Lost {
        agent_id: AgentId,
    },
}

// ---------------------------------------------------------------------------
// Static Discovery
// ---------------------------------------------------------------------------

const STATIC_RERESOLUTION_INTERVAL: Duration = Duration::from_secs(60);

pub async fn run_static_discovery(
    peers: Vec<PersistedStaticPeerConfig>,
    tx: mpsc::Sender<PeerEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    if peers.is_empty() {
        cancel.cancelled().await;
        return Ok(());
    }

    let mut last_addrs: HashMap<AgentId, SocketAddr> = HashMap::new();

    // Initial resolution and emission
    for peer in &peers {
        match peer.addr.resolve_for_config_load().await {
            Ok(addr) => {
                last_addrs.insert(peer.agent_id.clone(), addr);
                tx.send(PeerEvent::Discovered {
                    agent_id: peer.agent_id.clone(),
                    addr,
                    pubkey: peer.pubkey.clone(),
                })
                .await
                .map_err(|_| anyhow::anyhow!("peer event channel closed"))?;
            }
            Err(err) => {
                warn!(
                    agent_id = %peer.agent_id,
                    addr = %peer.addr,
                    error = %err,
                    "failed to resolve static peer address"
                );
            }
        }
    }

    // Periodic re-resolution loop
    let mut interval = tokio::time::interval(STATIC_RERESOLUTION_INTERVAL);
    interval.tick().await; // consume the first immediate tick

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = interval.tick() => {
                for peer in &peers {
                    match peer.addr.resolve_for_config_load().await {
                        Ok(addr) => {
                            if last_addrs.get(&peer.agent_id) != Some(&addr) {
                                debug!(
                                    agent_id = %peer.agent_id,
                                    old_addr = ?last_addrs.get(&peer.agent_id),
                                    new_addr = %addr,
                                    "static peer address changed"
                                );
                                last_addrs.insert(peer.agent_id.clone(), addr);
                                if tx.send(PeerEvent::Discovered {
                                    agent_id: peer.agent_id.clone(),
                                    addr,
                                    pubkey: peer.pubkey.clone(),
                                }).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                        Err(err) => {
                            warn!(
                                agent_id = %peer.agent_id,
                                addr = %peer.addr,
                                error = %err,
                                "hostname re-resolution failed, retaining last-known address"
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// mDNS Discovery
// ---------------------------------------------------------------------------

pub async fn run_mdns_discovery(
    local_agent_id: AgentId,
    local_pubkey: String,
    port: u16,
    tx: mpsc::Sender<PeerEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    let mdns = ServiceDaemon::new().context("failed to start mDNS daemon")?;

    let instance_name = format!("axon-{}", local_agent_id);
    let hostname = format!("{instance_name}.local.");

    let properties = [
        ("agent_id", local_agent_id.as_str()),
        ("pubkey", local_pubkey.as_str()),
    ];

    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name,
        &hostname,
        "",
        port,
        &properties[..],
    )
    .context("failed to build mDNS service info")?;

    mdns.register(service)
        .context("failed to register mDNS advertisement")?;

    let receiver = mdns
        .browse(SERVICE_TYPE)
        .context("failed to start mDNS browse")?;

    let mut fullname_to_agent_id = HashMap::<String, AgentId>::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = receiver.recv_async() => {
                let event = match event {
                    Ok(event) => event,
                    Err(err) => {
                        warn!(error = %err, "mDNS browse channel closed");
                        break;
                    }
                };

                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        match parse_resolved_service(&local_agent_id, &info) {
                            Ok(Some((peer_event, fullname, agent_id))) => {
                                fullname_to_agent_id.insert(fullname, agent_id);
                                if tx.send(peer_event).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(err) => {
                                warn!(error = %err, "failed to parse discovered mDNS service");
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                        if let Some(agent_id) = fullname_to_agent_id.remove(&fullname)
                            && tx.send(PeerEvent::Lost { agent_id }).await.is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        debug!(event = ?other, "ignoring non-resolved mDNS event");
                    }
                }
            }
        }
    }

    let _ = mdns.shutdown();
    Ok(())
}

fn parse_resolved_service(
    local_agent_id: &str,
    info: &ServiceInfo,
) -> Result<Option<(PeerEvent, String, AgentId)>> {
    let Some(agent_id) = info.get_property_val_str("agent_id") else {
        return Ok(None);
    };
    if agent_id == local_agent_id {
        return Ok(None);
    }

    let Some(pubkey) = info.get_property_val_str("pubkey") else {
        return Ok(None);
    };

    let Some(ip) = preferred_ip(info) else {
        return Ok(None);
    };

    let addr = SocketAddr::new(ip, info.get_port());
    let agent_id = AgentId::from(agent_id);
    let event = PeerEvent::Discovered {
        agent_id: agent_id.clone(),
        addr,
        pubkey: pubkey.to_string(),
    };

    Ok(Some((event, info.get_fullname().to_string(), agent_id)))
}

fn preferred_ip(info: &ServiceInfo) -> Option<IpAddr> {
    let mut v4 = None;
    let mut v6 = None;

    for ip in info.get_addresses() {
        match ip {
            IpAddr::V4(ipv4) if !ipv4.is_loopback() => {
                v4 = Some(IpAddr::V4(*ipv4));
                break;
            }
            IpAddr::V6(ipv6) if !ipv6.is_loopback() => {
                v6 = Some(IpAddr::V6(*ipv6));
            }
            _ => {}
        }
    }

    v4.or(v6)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
