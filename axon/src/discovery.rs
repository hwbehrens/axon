use anyhow::Result;
use base64::Engine;
use std::net::SocketAddr;
use tokio::sync::mpsc;

pub enum PeerEvent {
    Discovered {
        agent_id: String,
        addr: SocketAddr,
        pubkey: Vec<u8>,
    },
    Lost {
        agent_id: String,
    },
}

pub trait Discovery: Send + Sync {
    async fn run(self: Box<Self>, tx: mpsc::Sender<PeerEvent>) -> Result<()>;
}

pub struct MdnsDiscovery {
    agent_id: String,
    pubkey: Vec<u8>,
    port: u16,
}

impl MdnsDiscovery {
    pub fn new(agent_id: String, pubkey: Vec<u8>, port: u16) -> Self {
        Self { agent_id, pubkey, port }
    }
}

impl Discovery for MdnsDiscovery {
    async fn run(self: Box<Self>, tx: mpsc::Sender<PeerEvent>) -> Result<()> {
        use mdns_sd::{ServiceDaemon, ServiceInfo};
        use std::collections::HashMap;

        let mdns = ServiceDaemon::new()?;
        let service_type = "_axon._udp.local.";
        
        let mut properties = HashMap::new();
        properties.insert("agent_id".to_string(), self.agent_id.clone());
        properties.insert("pubkey".to_string(), base64::engine::general_purpose::STANDARD.encode(&self.pubkey));

        let service_info = ServiceInfo::new(
            service_type,
            &self.agent_id,
            &format!("{}.local.", self.agent_id),
            "",
            self.port,
            Some(properties),
        )?;

        mdns.register(service_info)?;

        let receiver = mdns.browse(service_type)?;
        
        while let Ok(event) = receiver.recv_async().await {
            match event {
                mdns_sd::ServiceEvent::ServiceResolved(info) => {
                    let agent_id = info.get_property_val_str("agent_id");
                    let pubkey_base64 = info.get_property_val_str("pubkey");
                    
                    if let (Some(id), Some(pk_b64)) = (agent_id, pubkey_base64) {
                        if let Ok(pk) = base64::engine::general_purpose::STANDARD.decode(pk_b64) {
                            if let Some(addr) = info.get_addresses().iter().next() {
                                let socket_addr = SocketAddr::new(*addr, info.get_port());
                                let _ = tx.send(PeerEvent::Discovered {
                                    agent_id: id.to_string(),
                                    addr: socket_addr,
                                    pubkey: pk,
                                }).await;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }
}
