use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::message::AgentId;
use crate::peer_directory::{ObservationId, ObservationSource, PeerObservation};

pub const SERVICE_TYPE: &str = "_axon._udp.local.";

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    Observed(PeerObservation),
    Lost(ObservationId),
}

pub async fn run_mdns_discovery(
    local_agent_id: AgentId,
    local_pubkey: String,
    port: u16,
    tx: mpsc::Sender<DiscoveryEvent>,
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
    .context("failed to build mDNS service info")?
    .enable_addr_auto();
    mdns.register(service)
        .context("failed to register mDNS advertisement")?;
    let receiver = mdns
        .browse(SERVICE_TYPE)
        .context("failed to start mDNS browse")?;
    let mut observations_by_service = HashMap::<String, BTreeSet<ObservationId>>::new();

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
                        let fullname = info.get_fullname().to_string();
                        match parse_resolved_service(&local_agent_id, &info) {
                            Ok(observations) => {
                                let next_ids: BTreeSet<_> = observations
                                    .iter()
                                    .map(|observation| observation.id.clone())
                                    .collect();
                                let previous = observations_by_service
                                    .insert(fullname, next_ids.clone())
                                    .unwrap_or_default();
                                for stale in previous.difference(&next_ids) {
                                    if tx.send(DiscoveryEvent::Lost(stale.clone())).await.is_err() {
                                        return Ok(());
                                    }
                                }
                                for observation in observations {
                                    if tx.send(DiscoveryEvent::Observed(observation)).await.is_err() {
                                        return Ok(());
                                    }
                                }
                            }
                            Err(err) => warn!(error = %err, "rejected invalid mDNS observation"),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                        if let Some(ids) = observations_by_service.remove(&fullname) {
                            for id in ids {
                                if tx.send(DiscoveryEvent::Lost(id)).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    other => debug!(event = ?other, "ignoring non-resolved mDNS event"),
                }
            }
        }
    }

    let _ = mdns.shutdown();
    Ok(())
}

fn parse_resolved_service(
    local_agent_id: &AgentId,
    info: &ServiceInfo,
) -> Result<Vec<PeerObservation>> {
    let Some(agent_id_raw) = info.get_property_val_str("agent_id") else {
        return Ok(Vec::new());
    };
    let agent_id = AgentId::parse(agent_id_raw).context("invalid advertised Agent ID")?;
    if &agent_id == local_agent_id {
        return Ok(Vec::new());
    }
    let Some(public_key) = info.get_property_val_str("pubkey") else {
        return Ok(Vec::new());
    };
    let display_name = service_display_name(info.get_fullname());
    let mut addresses: Vec<IpAddr> = info
        .get_addresses()
        .iter()
        .copied()
        .filter(|address| !address.is_loopback())
        .collect();
    addresses.sort_by_key(|address| (!address.is_ipv4(), *address));
    addresses.dedup();

    addresses
        .into_iter()
        .map(|address| {
            let endpoint = std::net::SocketAddr::new(address, info.get_port());
            let id = ObservationId::new(format!("mdns:{}:{endpoint}", info.get_fullname()))?;
            PeerObservation::new(
                id,
                agent_id.clone(),
                public_key,
                Some(endpoint),
                display_name.clone(),
                ObservationSource::Mdns,
            )
        })
        .collect()
}

fn service_display_name(fullname: &str) -> Option<Box<str>> {
    fullname
        .strip_suffix(SERVICE_TYPE)
        .map(|value| value.trim_end_matches('.'))
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string().into_boxed_str())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
