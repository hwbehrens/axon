//! Bonjour/mDNS peer discovery.
//!
//! # Dependency behavior (`mdns-sd` 0.21.5)
//!
//! The dependency emits [`ServiceEvent::ServiceResolved`] only when a record
//! is new or changes. A healthy peer that keeps re-announcing unchanged
//! records therefore produces no further events, so AXON cannot infer
//! liveness from resolved events alone; membership in the dependency's record
//! cache is the real liveness signal, and that cache is not exposed. This
//! module compensates by re-emitting its last-known observation set every
//! [`MDNS_REFRESH_INTERVAL`] — strictly shorter than the peer directory's
//! stale timeout — with a fresh `observed_at`. `ServiceRemoved` (which the
//! dependency emits when a PTR or the last SRV record expires, or a
//! re-resolved instance becomes invalid) still closes the loop with `Lost`,
//! so a withdrawn peer disappears on the TTL boundary.
//!
//! The dependency's DNS record cache is neither bounded nor inspectable, so
//! AXON cannot evict hostile entries from it. AXON bounds only what it tracks
//! and forwards: [`MAX_TRACKED_SERVICES`] service instances and at most
//! [`MAX_OBSERVATIONS_PER_PEER`] live observations per advertised Agent ID.
//! Malformed advertisements are rejected with debug-level logging rather than
//! per-record warnings, because a hostile or misbehaving link can generate
//! them faster than any human can read.

mod record;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::discovery::record::ResolvedRecord;
use crate::message::AgentId;
use crate::peer_directory::{
    MAX_OBSERVATIONS_PER_PEER, ObservationId, ObservationSource, PeerObservation,
};

pub const SERVICE_TYPE: &str = "_axon._udp.local.";

/// Liveness refresh cadence for unchanged mDNS records.
///
/// Must stay below the peer directory's `OBSERVATION_STALE_TIMEOUT` (60s) so
/// a live observation is re-reported before the 5-second stale sweep can
/// expire it. 30s leaves one full sweep in reserve.
const MDNS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// Bound on waiting for the unregister (goodbye) and shutdown
/// acknowledgements at teardown, matching the daemon's small child-task join
/// budget.
const MDNS_TEARDOWN_ACK_TIMEOUT: Duration = Duration::from_millis(500);

/// Maximum number of mDNS service instances tracked for stale-observation
/// diffing. Hostile or misbehaving networks could otherwise grow this map
/// indefinitely by announcing unique instance names that never emit
/// `ServiceRemoved`. Eviction is oldest-entry and emits `Lost` so the peer
/// directory expires the corresponding candidates.
pub const MAX_TRACKED_SERVICES: usize = 1024;

/// Maximum bytes retained from an advertised service instance name.
///
/// Instance names are attacker-controlled. Display names flow into peer
/// tables, logs, and IPC views, so they are stripped of control characters
/// and length-bounded before any consumer sees them.
const MAX_DISPLAY_NAME_BYTES: usize = 128;

/// Diffing state for mDNS browse results: the live observation set per
/// service name, the observation IDs owned by each advertised Agent ID
/// (enforcing the peer directory's per-peer capacity before events are
/// emitted), and the insertion order backing oldest-entry eviction.
///
/// Both fan-out dimensions are bounded: the number of service names by
/// [`MAX_TRACKED_SERVICES`], and the number of live observations per Agent ID
/// by [`MAX_OBSERVATIONS_PER_PEER`]. The order queue is kept in exact sync
/// with the service map — it holds each live name exactly once, oldest first
/// — so refreshes are deterministic and cannot double-report a service.
struct ServiceTracker {
    observations_by_service: HashMap<String, BTreeMap<ObservationId, PeerObservation>>,
    observations_by_agent: HashMap<AgentId, BTreeSet<ObservationId>>,
    insertion_order: VecDeque<String>,
}

impl ServiceTracker {
    fn new() -> Self {
        Self {
            observations_by_service: HashMap::new(),
            observations_by_agent: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    /// Record a resolved service and produce the events to forward.
    /// `Lost` events (evictions, stale observations, and budget drops)
    /// precede `Observed` events so consumers expire before they re-learn.
    fn observe_resolution(
        &mut self,
        fullname: String,
        observations: Vec<PeerObservation>,
    ) -> Vec<DiscoveryEvent> {
        let mut events = Vec::new();
        // Whether this name is NEWLY tracked, computed BEFORE the insert:
        // an empty previous set cannot play this role because a refresh of a
        // known name whose stored set is empty (self/malformed/address-less
        // services refresh periodically) also yields an empty previous set.
        let newly_tracked = !self.observations_by_service.contains_key(&fullname);
        if newly_tracked && self.observations_by_service.len() >= MAX_TRACKED_SERVICES {
            // Bound the tracking map independently of peer-directory limits:
            // when a NEW name arrives at capacity, evict the OLDEST-INSERTED
            // live service so an active peer is never evicted ahead of stale
            // ones purely by hash-map ordering.
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
                debug!(
                    service = %sanitize_display_name(&evicted),
                    capacity = MAX_TRACKED_SERVICES,
                    "mDNS service tracking at capacity; evicting oldest"
                );
                let previous = self
                    .observations_by_service
                    .remove(&evicted)
                    .unwrap_or_default();
                self.unindex(&previous);
                events.extend(previous.into_keys().map(DiscoveryEvent::Lost));
            }
        }

        let previous = self
            .observations_by_service
            .remove(&fullname)
            .unwrap_or_default();
        // Unindex the previous set first: a service's own observations must
        // not consume its Agent ID's budget while it refreshes.
        self.unindex(&previous);
        let previous_ids: BTreeSet<_> = previous.keys().cloned().collect();

        let mut kept = BTreeMap::new();
        let mut kept_ids = BTreeSet::new();
        let mut dropped = 0usize;
        for observation in observations {
            let agent_id = observation.identity.agent_id().clone();
            let used = self
                .observations_by_agent
                .get(&agent_id)
                .map_or(0, BTreeSet::len);
            if used >= MAX_OBSERVATIONS_PER_PEER {
                dropped += 1;
                continue;
            }
            self.observations_by_agent
                .entry(agent_id)
                .or_default()
                .insert(observation.id.clone());
            kept_ids.insert(observation.id.clone());
            kept.insert(observation.id.clone(), observation);
        }
        if dropped > 0 {
            debug!(
                service = %sanitize_display_name(&fullname),
                dropped,
                cap = MAX_OBSERVATIONS_PER_PEER,
                "mDNS observation budget reached; extra endpoints dropped"
            );
        }

        events.extend(
            previous_ids
                .difference(&kept_ids)
                .cloned()
                .map(DiscoveryEvent::Lost),
        );
        events.extend(kept.values().cloned().map(DiscoveryEvent::Observed));
        self.observations_by_service.insert(fullname.clone(), kept);
        if newly_tracked {
            self.insertion_order.push_back(fullname);
        }
        events
    }

    /// Drop a service after `ServiceRemoved`, emitting `Lost` for its live
    /// observations and purging its order-queue entry so a later re-resolve
    /// is not reported twice by [`Self::refresh_events`].
    fn remove_service(&mut self, fullname: &str) -> Vec<DiscoveryEvent> {
        let removed = self
            .observations_by_service
            .remove(fullname)
            .unwrap_or_default();
        self.unindex(&removed);
        self.insertion_order.retain(|name| name != fullname);
        removed.into_keys().map(DiscoveryEvent::Lost).collect()
    }

    /// Re-emit every tracked observation with a fresh `observed_at`.
    ///
    /// `mdns-sd` 0.21.5 does not re-emit `ServiceResolved` for unchanged
    /// records, so without this the peer directory's stale sweep would expire
    /// a live peer and never re-learn it until its records change. Walking
    /// `insertion_order` (kept in exact sync with the service map) makes the
    /// output order deterministic.
    fn refresh_events(&self, now: Instant) -> Vec<DiscoveryEvent> {
        self.insertion_order
            .iter()
            .filter_map(|fullname| self.observations_by_service.get(fullname))
            .flat_map(|observations| observations.values())
            .map(|observation| {
                let mut refreshed = observation.clone();
                refreshed.observed_at = now;
                DiscoveryEvent::Observed(refreshed)
            })
            .collect()
    }

    /// Remove observations from the per-Agent-ID index, dropping empty sets.
    fn unindex(&mut self, observations: &BTreeMap<ObservationId, PeerObservation>) {
        for observation in observations.values() {
            let agent_id = observation.identity.agent_id();
            if let Some(ids) = self.observations_by_agent.get_mut(agent_id) {
                ids.remove(&observation.id);
                if ids.is_empty() {
                    self.observations_by_agent.remove(agent_id);
                }
            }
        }
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
    let advertisement = service.get_fullname().to_string();
    mdns.register(service)
        .context("failed to register mDNS advertisement")?;
    let receiver = mdns
        .browse(SERVICE_TYPE)
        .context("failed to start mDNS browse")?;
    let mut tracker = ServiceTracker::new();
    let mut refresh_interval = tokio::time::interval(MDNS_REFRESH_INTERVAL);
    refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    'run: loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = refresh_interval.tick() => {
                // Unchanged records never produce another `ServiceResolved`
                // (mdns-sd 0.21.5); replaying the cached observations with a
                // fresh timestamp is what keeps live peers from expiring.
                for event in tracker.refresh_events(Instant::now()) {
                    if tx.send(event).await.is_err() {
                        break 'run;
                    }
                }
            }
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
                        match parse_resolved_service(&local_agent_id, info.as_ref()) {
                            Ok(observations) => {
                                for event in tracker.observe_resolution(fullname, observations) {
                                    if tx.send(event).await.is_err() {
                                        break 'run;
                                    }
                                }
                            }
                            // Hostile advertisements are common on a busy or
                            // adversarial link; keep the reason debuggable
                            // without per-record default-level spam.
                            Err(err) => debug!(error = %err, "rejected invalid mDNS observation"),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                        for event in tracker.remove_service(&fullname) {
                            if tx.send(event).await.is_err() {
                                break 'run;
                            }
                        }
                    }
                    other => debug!(event = ?other, "ignoring non-resolved mDNS event"),
                }
            }
        }
    }

    withdraw_and_shutdown(&mdns, &advertisement).await;
    Ok(())
}

/// Withdraw this daemon's advertisement and stop the mDNS daemon cleanly.
///
/// `mdns-sd` multicasts a goodbye packet while handling `unregister`, so
/// awaiting the acknowledgement is what guarantees the withdrawal actually
/// happened before the sockets close. Both waits are bounded: daemon shutdown
/// only budgets a few seconds for child tasks, and a missing acknowledgement
/// must never wedge it.
async fn withdraw_and_shutdown(mdns: &ServiceDaemon, fullname: &str) {
    match mdns.unregister(fullname) {
        Ok(ack) => {
            if tokio::time::timeout(MDNS_TEARDOWN_ACK_TIMEOUT, ack.recv_async())
                .await
                .is_err()
            {
                debug!(
                    service = %sanitize_display_name(fullname),
                    "timed out waiting for mDNS unregister acknowledgement"
                );
            }
        }
        Err(err) => debug!(error = %err, "mDNS unregister request rejected"),
    }
    match mdns.shutdown() {
        Ok(status) => {
            if tokio::time::timeout(MDNS_TEARDOWN_ACK_TIMEOUT, status.recv_async())
                .await
                .is_err()
            {
                debug!("timed out waiting for mDNS daemon shutdown acknowledgement");
            }
        }
        Err(err) => debug!(error = %err, "mDNS shutdown request rejected"),
    }
}

fn parse_resolved_service(
    local_agent_id: &AgentId,
    info: &impl ResolvedRecord,
) -> Result<Vec<PeerObservation>> {
    let Some(agent_id_raw) = info.property("agent_id") else {
        return Ok(Vec::new());
    };
    let agent_id = AgentId::parse(agent_id_raw).context("invalid advertised Agent ID")?;
    if &agent_id == local_agent_id {
        return Ok(Vec::new());
    }
    let Some(public_key) = info.property("pubkey") else {
        return Ok(Vec::new());
    };
    let display_name = service_display_name(info.fullname());
    let mut addresses: Vec<IpAddr> = info
        .addresses()
        .into_iter()
        .filter(is_dialable_candidate)
        .collect();
    addresses.sort_by_key(|address| (!address.is_ipv4(), *address));
    addresses.dedup();
    // The peer directory retains at most MAX_OBSERVATIONS_PER_PEER live
    // observations per peer; reporting more can only produce rejection
    // warnings. IPv4 sorts first, so the most broadly dialable endpoints
    // survive when a multi-homed host overflows the bound.
    addresses.truncate(MAX_OBSERVATIONS_PER_PEER);

    addresses
        .into_iter()
        .map(|address| {
            let endpoint = std::net::SocketAddr::new(address, info.port());
            let id = ObservationId::new(format!("mdns:{}:{endpoint}", info.fullname()))?;
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

/// Whether an advertised address can be used as a QUIC dial target.
///
/// Loopback addresses only describe the advertising host itself. Unscoped
/// IPv6 link-local addresses are not dialable either: they require an
/// interface scope that DNS-SD records cannot carry, so retaining them only
/// produces failed dial attempts and warnings.
fn is_dialable_candidate(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => !address.is_loopback(),
        IpAddr::V6(address) => !address.is_loopback() && !address.is_unicast_link_local(),
    }
}

fn service_display_name(fullname: &str) -> Option<Box<str>> {
    fullname
        .strip_suffix(SERVICE_TYPE)
        .map(|value| value.trim_end_matches('.'))
        .filter(|value| !value.is_empty())
        .map(sanitize_display_name)
        .map(String::into_boxed_str)
}

/// Sanitize an attacker-controlled instance name for tables, logs, and IPC
/// views: control characters become U+FFFD and the value is truncated on a
/// UTF-8 boundary to [`MAX_DISPLAY_NAME_BYTES`].
fn sanitize_display_name(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len().min(MAX_DISPLAY_NAME_BYTES));
    for ch in raw.chars() {
        let ch = if ch.is_control() { '\u{FFFD}' } else { ch };
        if sanitized.len() + ch.len_utf8() > MAX_DISPLAY_NAME_BYTES {
            break;
        }
        sanitized.push(ch);
    }
    sanitized
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
