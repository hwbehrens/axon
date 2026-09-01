use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use crate::message::AgentId;

use super::store::StoredPeer;
use super::{ObservationId, PeerIdentity, PeerLocator, PeerTrust, PeerView, PinningSnapshot};

#[derive(Debug, Clone, Default)]
pub(super) struct DirectoryState {
    pub(super) enrolled: BTreeMap<AgentId, EnrolledPeer>,
    pub(super) candidates: BTreeMap<AgentId, CandidatePeer>,
    pub(super) observation_index: HashMap<ObservationId, AgentId>,
}

#[derive(Debug, Clone)]
pub(super) struct EnrolledPeer {
    pub(super) identity: PeerIdentity,
    pub(super) locators: BTreeSet<PeerLocator>,
    pub(super) observations: BTreeMap<ObservationId, LiveObservation>,
}

#[derive(Debug, Clone)]
pub(super) struct CandidatePeer {
    pub(super) identity: PeerIdentity,
    pub(super) observations: BTreeMap<ObservationId, LiveObservation>,
}

#[derive(Debug, Clone)]
pub(super) struct LiveObservation {
    pub(super) endpoint: Option<SocketAddr>,
    pub(super) display_name: Option<Box<str>>,
    pub(super) observed_at: Instant,
    pub(super) conflicted: bool,
}

impl DirectoryState {
    // Identity-conflict checks live at the PeerIdentity constructor: every
    // construction path derives the Agent ID from the public key, so two
    // identities sharing an Agent ID necessarily share a key. A runtime
    // comparison here would be provably constant-false.

    pub(super) fn recompute_conflicts(&mut self) {
        let mut owners = HashMap::<SocketAddr, BTreeSet<AgentId>>::new();
        for (agent_id, peer) in &self.enrolled {
            for endpoint in peer.observations.values().filter_map(|item| item.endpoint) {
                owners.entry(endpoint).or_default().insert(agent_id.clone());
            }
        }
        for (agent_id, peer) in &self.candidates {
            for endpoint in peer.observations.values().filter_map(|item| item.endpoint) {
                owners.entry(endpoint).or_default().insert(agent_id.clone());
            }
        }
        for peer in self.enrolled.values_mut() {
            for observation in peer.observations.values_mut() {
                observation.conflicted = observation
                    .endpoint
                    .is_some_and(|endpoint| owners.get(&endpoint).is_some_and(|set| set.len() > 1));
            }
        }
        for peer in self.candidates.values_mut() {
            for observation in peer.observations.values_mut() {
                observation.conflicted = observation
                    .endpoint
                    .is_some_and(|endpoint| owners.get(&endpoint).is_some_and(|set| set.len() > 1));
            }
        }
    }

    pub(super) fn remove_observation(&mut self, id: &ObservationId) -> Option<AgentId> {
        let agent_id = self.observation_index.remove(id)?;
        if let Some(peer) = self.enrolled.get_mut(&agent_id) {
            peer.observations.remove(id);
        }
        if let Some(peer) = self.candidates.get_mut(&agent_id) {
            peer.observations.remove(id);
            if peer.observations.is_empty() {
                self.candidates.remove(&agent_id);
            }
        }
        Some(agent_id)
    }

    pub(super) fn observation(&self, id: &ObservationId) -> Option<&LiveObservation> {
        let agent_id = self.observation_index.get(id)?;
        self.enrolled
            .get(agent_id)
            .and_then(|peer| peer.observations.get(id))
            .or_else(|| {
                self.candidates
                    .get(agent_id)
                    .and_then(|peer| peer.observations.get(id))
            })
    }

    /// Structural consistency: every `observation_index` entry must resolve
    /// to a live observation in an enrolled or candidate record, and every
    /// recorded observation must be indexed.
    ///
    /// Ghost entries (index without record) are invisible to
    /// `expire_observations` — `observation()` no longer resolves them — so
    /// nothing else ever removes them. Pinned by the Hegel invariants and
    /// the revocation-interleaving tests (DEC-022).
    #[cfg(test)]
    pub(super) fn assert_no_ghost_observations(&self) {
        for (id, agent_id) in &self.observation_index {
            let resolves = self
                .enrolled
                .get(agent_id)
                .is_some_and(|peer| peer.observations.contains_key(id))
                || self
                    .candidates
                    .get(agent_id)
                    .is_some_and(|peer| peer.observations.contains_key(id));
            assert!(
                resolves,
                "ghost observation {id} owned by {agent_id} has no live record"
            );
        }
        for (agent_id, peer) in &self.enrolled {
            for id in peer.observations.keys() {
                assert_eq!(
                    self.observation_index.get(id),
                    Some(agent_id),
                    "enrolled observation {id} lost its index entry"
                );
            }
        }
        for (agent_id, peer) in &self.candidates {
            for id in peer.observations.keys() {
                assert_eq!(
                    self.observation_index.get(id),
                    Some(agent_id),
                    "candidate observation {id} lost its index entry"
                );
            }
        }
    }

    pub(super) fn stored_peers(&self) -> Vec<StoredPeer> {
        self.enrolled
            .values()
            .map(|peer| StoredPeer {
                agent_id: peer.identity.agent_id().clone(),
                public_key: peer.identity.public_key().to_string(),
                locators: peer.locators.iter().cloned().collect(),
            })
            .collect()
    }

    pub(super) fn pinning_snapshot(&self) -> PinningSnapshot {
        Arc::new(
            self.enrolled
                .values()
                .map(|peer| {
                    (
                        peer.identity.agent_id().to_string(),
                        peer.identity.public_key().to_string(),
                    )
                })
                .collect(),
        )
    }

    pub(super) fn views(&self) -> Vec<PeerView> {
        let enrolled = self.enrolled.values().map(|peer| PeerView {
            identity: peer.identity.clone(),
            trust: PeerTrust::Enrolled,
            configured_locators: peer.locators.iter().cloned().collect(),
            observed_endpoints: live_endpoints(&peer.observations),
            display_name: latest_display_name(&peer.observations),
        });
        let candidates = self.candidates.values().map(|peer| PeerView {
            identity: peer.identity.clone(),
            trust: PeerTrust::Candidate,
            configured_locators: Vec::new(),
            observed_endpoints: live_endpoints(&peer.observations),
            display_name: latest_display_name(&peer.observations),
        });
        enrolled.chain(candidates).collect()
    }
}

fn live_endpoints(observations: &BTreeMap<ObservationId, LiveObservation>) -> Vec<SocketAddr> {
    observations
        .values()
        .filter(|observation| !observation.conflicted)
        .filter_map(|observation| observation.endpoint)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn latest_display_name(
    observations: &BTreeMap<ObservationId, LiveObservation>,
) -> Option<Box<str>> {
    observations
        .values()
        .filter_map(|observation| {
            observation
                .display_name
                .as_ref()
                .map(|name| (observation.observed_at, name.clone()))
        })
        .max_by_key(|(observed_at, _)| *observed_at)
        .map(|(_, name)| name)
}
