use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result};
use async_trait::async_trait;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::config::StaticPeerConfig;

pub const SERVICE_TYPE: &str = "_axon._udp.local.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    Discovered {
        agent_id: String,
        addr: SocketAddr,
        pubkey: String,
    },
    Lost {
        agent_id: String,
    },
}

#[async_trait]
pub trait Discovery: Send + Sync {
    async fn run(&self, tx: mpsc::Sender<PeerEvent>, cancel: CancellationToken) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Static Discovery
// ---------------------------------------------------------------------------

pub struct StaticDiscovery {
    peers: Vec<StaticPeerConfig>,
}

impl StaticDiscovery {
    pub fn new(peers: Vec<StaticPeerConfig>) -> Self {
        Self { peers }
    }
}

#[async_trait]
impl Discovery for StaticDiscovery {
    async fn run(&self, tx: mpsc::Sender<PeerEvent>, cancel: CancellationToken) -> Result<()> {
        for peer in &self.peers {
            tx.send(PeerEvent::Discovered {
                agent_id: peer.agent_id.clone(),
                addr: peer.addr,
                pubkey: peer.pubkey.clone(),
            })
            .await
            .map_err(|_| anyhow::anyhow!("peer event channel closed"))?;
        }
        // Stay alive until cancellation — replaces std::future::pending().
        cancel.cancelled().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// mDNS Discovery
// ---------------------------------------------------------------------------

pub struct MdnsDiscovery {
    local_agent_id: String,
    local_pubkey: String,
    port: u16,
}

impl MdnsDiscovery {
    pub fn new(local_agent_id: String, local_pubkey: String, port: u16) -> Self {
        Self {
            local_agent_id,
            local_pubkey,
            port,
        }
    }
}

#[async_trait]
impl Discovery for MdnsDiscovery {
    async fn run(&self, tx: mpsc::Sender<PeerEvent>, cancel: CancellationToken) -> Result<()> {
        let mdns = ServiceDaemon::new().context("failed to start mDNS daemon")?;

        let instance_name = format!("axon-{}", self.local_agent_id);
        let hostname = format!("{instance_name}.local.");

        let properties = [
            ("agent_id", self.local_agent_id.as_str()),
            ("pubkey", self.local_pubkey.as_str()),
        ];

        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &hostname,
            "",
            self.port,
            &properties[..],
        )
        .context("failed to build mDNS service info")?;

        mdns.register(service)
            .context("failed to register mDNS advertisement")?;

        let receiver = mdns
            .browse(SERVICE_TYPE)
            .context("failed to start mDNS browse")?;

        let mut fullname_to_agent_id = HashMap::<String, String>::new();

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
                            match parse_resolved_service(&self.local_agent_id, &info) {
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
}

fn parse_resolved_service(
    local_agent_id: &str,
    info: &ServiceInfo,
) -> Result<Option<(PeerEvent, String, String)>> {
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
    let event = PeerEvent::Discovered {
        agent_id: agent_id.to_string(),
        addr,
        pubkey: pubkey.to_string(),
    };

    Ok(Some((
        event,
        info.get_fullname().to_string(),
        agent_id.to_string(),
    )))
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
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_discovery_emits_all_peers() {
        let peers = vec![
            StaticPeerConfig {
                agent_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                addr: "127.0.0.1:7100".parse().expect("addr"),
                pubkey: "Zm9v".to_string(),
            },
            StaticPeerConfig {
                agent_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
                addr: "127.0.0.1:7101".parse().expect("addr"),
                pubkey: "YmFy".to_string(),
            },
        ];

        let discovery = StaticDiscovery::new(peers);
        let (tx, mut rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();

        tokio::spawn(async move {
            let _ = discovery.run(tx, cancel).await;
        });

        let first = rx.recv().await.expect("first event");
        let second = rx.recv().await.expect("second event");

        match first {
            PeerEvent::Discovered { agent_id, .. } => {
                assert_eq!(agent_id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            }
            _ => panic!("expected Discovered"),
        }
        match second {
            PeerEvent::Discovered { agent_id, .. } => {
                assert_eq!(agent_id, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
            }
            _ => panic!("expected Discovered"),
        }
    }

    #[tokio::test]
    async fn static_discovery_stays_alive() {
        let peers = vec![StaticPeerConfig {
            agent_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            addr: "127.0.0.1:7100".parse().expect("addr"),
            pubkey: "Zm9v".to_string(),
        }];

        let discovery = StaticDiscovery::new(peers);
        let (tx, mut rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(async move { discovery.run(tx, cancel).await });

        rx.recv().await.expect("should receive event");
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert!(!handle.is_finished());
        handle.abort();
    }

    #[test]
    fn parse_resolved_ignores_self() {
        let props = [
            ("agent_id", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("pubkey", "Zm9v"),
        ];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "axon-a",
            "axon-a.local.",
            "10.1.1.10",
            7100,
            &props[..],
        )
        .expect("service info");

        let parsed =
            parse_resolved_service("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_resolved_extracts_peer() {
        let props = [
            ("agent_id", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            ("pubkey", "YmFy"),
        ];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "axon-b",
            "axon-b.local.",
            "10.1.1.11",
            7101,
            &props[..],
        )
        .expect("service info");

        let parsed =
            parse_resolved_service("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
        let (event, _fullname, agent_id) = parsed.expect("expected discovered peer");

        assert_eq!(agent_id, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        match event {
            PeerEvent::Discovered {
                agent_id,
                addr,
                pubkey,
            } => {
                assert_eq!(agent_id, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
                assert_eq!(addr.to_string(), "10.1.1.11:7101");
                assert_eq!(pubkey, "YmFy");
            }
            _ => panic!("expected Discovered"),
        }
    }

    #[test]
    fn parse_resolved_skips_missing_agent_id() {
        let props: [(&str, &str); 0] = [];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "axon-x",
            "axon-x.local.",
            "10.1.1.12",
            7100,
            &props[..],
        )
        .expect("service info");

        let parsed =
            parse_resolved_service("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_resolved_skips_missing_pubkey() {
        let props = [("agent_id", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "axon-b",
            "axon-b.local.",
            "10.1.1.13",
            7100,
            &props[..],
        )
        .expect("service info");

        let parsed =
            parse_resolved_service("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
        assert!(parsed.is_none());
    }

    #[test]
    fn peer_event_equality() {
        let a = PeerEvent::Discovered {
            agent_id: "abc".to_string(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "key".to_string(),
        };
        let b = PeerEvent::Discovered {
            agent_id: "abc".to_string(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "key".to_string(),
        };
        assert_eq!(a, b);

        let c = PeerEvent::Lost {
            agent_id: "abc".to_string(),
        };
        assert_ne!(a, c);
    }
}
