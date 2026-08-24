use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use crate::message::AgentId;

use super::store::StoredPeer;
use super::{
    MAX_OBSERVATIONS_PER_PEER, ObservationId, PeerIdentity, PeerLocator, PeerTrust, PeerView,
    PinningSnapshot,
};

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
    pub(super) fn identity_conflicts(&self, identity: &PeerIdentity) -> bool {
        self.enrolled
            .get(identity.agent_id())
            .map(|peer| peer.identity.public_key() != identity.public_key())
            .or_else(|| {
                self.candidates
                    .get(identity.agent_id())
                    .map(|peer| peer.identity.public_key() != identity.public_key())
            })
            .unwrap_or(false)
    }

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

pub(super) fn insert_observation(
    observations: &mut BTreeMap<ObservationId, LiveObservation>,
    id: ObservationId,
    observation: LiveObservation,
) -> bool {
    if !observations.contains_key(&id) && observations.len() >= MAX_OBSERVATIONS_PER_PEER {
        return false;
    }
    observations.insert(id, observation);
    true
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
