use std::collections::{BTreeSet, HashMap, VecDeque};
use std::net::IpAddr;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::message::AgentId;
use crate::peer_directory::{ObservationId, ObservationSource, PeerObservation};

pub const SERVICE_TYPE: &str = "_axon._udp.local.";

/// Maximum number of mDNS service instances tracked for stale-observation
/// diffing. Hostile or misbehaving networks could otherwise grow this map
/// indefinitely by announcing unique instance names that never emit
/// `ServiceRemoved`. Eviction is oldest-entry and emits `Lost` so the peer
/// directory expires the corresponding candidates.
pub const MAX_TRACKED_SERVICES: usize = 1024;

/// Diffing state for mDNS browse results: the live observation set per
/// service name plus the insertion order backing oldest-entry eviction.
/// Both structures are bounded: the map by [`MAX_TRACKED_SERVICES`], and the
/// order queue grows only when a NEW name enters the map (refreshes of a
/// known name — including names whose stored set is empty because the
/// service is self/malformed/address-less — never re-append).
struct ServiceTracker {
    observations_by_service: HashMap<String, BTreeSet<ObservationId>>,
    insertion_order: VecDeque<String>,
}

impl ServiceTracker {
    fn new() -> Self {
        Self {
            observations_by_service: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    /// Record a resolved service and produce the events to forward.
    /// `Lost` events (evictions and stale observations) precede `Observed`
    /// events so consumers expire before they re-learn.
    fn observe_resolution(
        &mut self,
        fullname: String,
        observations: Vec<PeerObservation>,
    ) -> Vec<DiscoveryEvent> {
        let next_ids: BTreeSet<_> = observations
            .iter()
            .map(|observation| observation.id.clone())
            .collect();
        // Whether this name is NEWLY tracked, computed BEFORE the insert:
        // `previous.is_empty()` cannot play this role because a refresh of a
        // known name whose stored set is empty (self/malformed/no-address
        // services refresh periodically) yields an empty previous set too.
        // Re-appending those would grow `insertion_order` without bound even
        // though the map itself is capped.
        let newly_tracked = !self.observations_by_service.contains_key(&fullname);
        if newly_tracked && self.observations_by_service.len() >= MAX_TRACKED_SERVICES {
            // Bound the tracking map independently of peer-directory limits:
            // when a NEW name arrives at capacity, evict the OLDEST-INSERTED
            // live service so an active peer is never evicted ahead of stale
            // ones purely by hash-map ordering. The order queue may hold
            // stale names from earlier removals; they are skipped here and
            // pruned by `remove_service` compaction.
            let evicted = loop {
                match self.insertion_order.pop_front() {
                    Some(name) if self.observations_by_service.contains_key(&name) => {
                        break Some(name);
                    }
                    Some(_) => continue,
                    None => break None,
                }
            };
            if let Some(evicted) = evicted {
                warn!(
                    service = %evicted,
                    capacity = MAX_TRACKED_SERVICES,
                    "mDNS service tracking at capacity; evicting oldest"
                );
                if let Some(ids) = self.observations_by_service.remove(&evicted) {
                    return ids
                        .into_iter()
                        .map(DiscoveryEvent::Lost)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .chain(self.observe_resolution(fullname, observations))
                        .collect();
                }
                return self.observe_resolution(fullname, observations);
            }
        }
        let mut events = Vec::new();
        let previous = self
            .observations_by_service
            .insert(fullname.clone(), next_ids.clone())
            .unwrap_or_default();
        if newly_tracked {
            self.insertion_order.push_back(fullname);
        }
        events.extend(
            previous
                .difference(&next_ids)
                .map(|stale| DiscoveryEvent::Lost(stale.clone())),
        );
        events.extend(observations.into_iter().map(DiscoveryEvent::Observed));
        events
    }

    /// Drop a service after `ServiceRemoved`, emitting `Lost` for its live
    /// observations. Opportunistic compaction: churning instance names must
    /// not grow the order queue without limit once removals leave stale
    /// entries behind.
    fn remove_service(&mut self, fullname: &str) -> Vec<DiscoveryEvent> {
        let lost = self
            .observations_by_service
            .remove(fullname)
            .map(|ids| {
                ids.into_iter()
                    .map(DiscoveryEvent::Lost)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if self.insertion_order.len() > self.observations_by_service.len() * 2 + 16 {
            self.insertion_order
                .retain(|name| self.observations_by_service.contains_key(name));
        }
        lost
    }
}

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
    let mut tracker = ServiceTracker::new();

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
                                for event in tracker.observe_resolution(fullname, observations) {
                                    if tx.send(event).await.is_err() {
                                        return Ok(());
                                    }
                                }
                            }
                            Err(err) => warn!(error = %err, "rejected invalid mDNS observation"),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                        for event in tracker.remove_service(&fullname) {
                            if tx.send(event).await.is_err() {
                                return Ok(());
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
