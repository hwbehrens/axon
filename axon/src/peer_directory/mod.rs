mod state;
mod store;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::RwLock;

use crate::message::AgentId;
use state::{CandidatePeer, DirectoryState, EnrolledPeer, LiveObservation};

pub use store::{PeerStore, StoredPeer};
pub use types::{
    DialTarget, ObservationId, ObservationSource, ObserveOutcome, PeerIdentity, PeerLocator,
    PeerObservation, PeerTrust, PeerView,
};

pub const MAX_ENROLLED_PEERS: usize = 256;
pub const MAX_CANDIDATE_PEERS: usize = 256;
pub const MAX_LOCATORS_PER_PEER: usize = 8;
pub const MAX_OBSERVATIONS_PER_PEER: usize = 16;
pub const OBSERVATION_STALE_TIMEOUT: Duration = Duration::from_secs(60);

pub type PinningSnapshot = Arc<BTreeMap<String, String>>;
pub type PinningSnapshotHandle = Arc<StdRwLock<PinningSnapshot>>;

#[derive(Debug, Clone)]
pub struct PeerDirectory {
    local_agent_id: AgentId,
    state: Arc<RwLock<DirectoryState>>,
    pins: PinningSnapshotHandle,
    store: PeerStore,
}

impl PeerDirectory {
    pub async fn load(local_agent_id: AgentId, store: PeerStore) -> Result<Self> {
        let mut state = DirectoryState::default();
        for peer in store.load().await? {
            let identity = PeerIdentity::from_parts(peer.agent_id.clone(), &peer.public_key)?;
            if peer.agent_id == local_agent_id {
                bail!("peer store cannot enroll the local Agent ID {local_agent_id}");
            }
            let locators: BTreeSet<_> = peer.locators.into_iter().collect();
            let record = EnrolledPeer {
                identity,
                locators,
                observations: BTreeMap::new(),
            };
            if state
                .enrolled
                .insert(peer.agent_id.clone(), record)
                .is_some()
            {
                bail!("peer store contains duplicate Agent ID {}", peer.agent_id);
            }
        }
        let pins = Arc::new(StdRwLock::new(state.pinning_snapshot()));
        Ok(Self {
            local_agent_id,
            state: Arc::new(RwLock::new(state)),
            pins,
            store,
        })
    }

    pub fn pinning_snapshot(&self) -> PinningSnapshotHandle {
        self.pins.clone()
    }

    pub async fn observe(&self, observation: PeerObservation) -> ObserveOutcome {
        if observation.identity.agent_id() == &self.local_agent_id {
            return ObserveOutcome::IgnoredSelf;
        }

        let mut state = self.state.write().await;
        if state
            .observation_index
            .get(&observation.id)
            .is_some_and(|existing| existing != observation.identity.agent_id())
        {
            return ObserveOutcome::IdentityConflict;
        }

        let is_enrolled = state.enrolled.contains_key(observation.identity.agent_id());
        let is_refresh = state
            .candidates
            .contains_key(observation.identity.agent_id());
        if !is_enrolled
            && !state
                .candidates
                .contains_key(observation.identity.agent_id())
            && state.candidates.len() >= MAX_CANDIDATE_PEERS
        {
            return ObserveOutcome::CapacityReached;
        }

        state.remove_observation(&observation.id);
        let live = LiveObservation {
            endpoint: observation.endpoint,
            display_name: observation.display_name,
            observed_at: observation.observed_at,
            conflicted: false,
        };
        let agent_id = observation.identity.agent_id().clone();
        // Capacity is checked before any entry is created or extended so a
        // rejected observation cannot leave an empty candidate behind.
        let at_observation_capacity = |peer: Option<&BTreeMap<ObservationId, LiveObservation>>| {
            peer.is_some_and(|observations| observations.len() >= MAX_OBSERVATIONS_PER_PEER)
        };
        if let Some(peer) = state.enrolled.get_mut(&agent_id) {
            if at_observation_capacity(Some(&peer.observations)) {
                return ObserveOutcome::CapacityReached;
            }
            peer.observations.insert(observation.id.clone(), live);
        } else if at_observation_capacity(
            state
                .candidates
                .get(&agent_id)
                .map(|peer| &peer.observations),
        ) {
            return ObserveOutcome::CapacityReached;
        } else {
            let peer = state
                .candidates
                .entry(agent_id.clone())
                .or_insert_with(|| CandidatePeer {
                    identity: observation.identity,
                    observations: BTreeMap::new(),
                });
            peer.observations.insert(observation.id.clone(), live);
        }
        state
            .observation_index
            .insert(observation.id.clone(), agent_id);
        state.recompute_conflicts();
        let conflicted = state
            .observation(&observation.id)
            .is_some_and(|observation| observation.conflicted);

        if conflicted {
            ObserveOutcome::LocatorConflict
        } else if is_enrolled {
            ObserveOutcome::EnrolledPeerRefreshed
        } else if is_refresh {
            ObserveOutcome::CandidateRefreshed
        } else {
            ObserveOutcome::CandidateAdded
        }
    }

    pub async fn remove_observation(&self, id: &ObservationId) {
        let mut state = self.state.write().await;
        state.remove_observation(id);
        state.recompute_conflicts();
    }

    pub async fn expire_observations(&self, now: Instant, ttl: Duration) -> Vec<AgentId> {
        let mut state = self.state.write().await;
        let expired: Vec<_> = state
            .observation_index
            .keys()
            .filter(|id| {
                state
                    .observation(id)
                    .is_some_and(|observation| now.duration_since(observation.observed_at) > ttl)
            })
            .cloned()
            .collect();
        let mut removed_candidates = Vec::new();
        for id in expired {
            if let Some(agent_id) = state.remove_observation(&id)
                && !state.candidates.contains_key(&agent_id)
            {
                removed_candidates.push(agent_id);
            }
        }
        state.recompute_conflicts();
        removed_candidates.sort();
        removed_candidates.dedup();
        removed_candidates
    }

    pub async fn enroll_candidate(&self, agent_id: &AgentId) -> Result<PeerIdentity> {
        let mut current = self.state.write().await;
        if let Some(peer) = current.enrolled.get(agent_id) {
            return Ok(peer.identity.clone());
        }
        let candidate = current.candidates.get(agent_id).cloned().ok_or_else(|| {
            anyhow::anyhow!("peer candidate {agent_id} is not currently observed")
        })?;
        if current.enrolled.len() >= MAX_ENROLLED_PEERS {
            bail!("enrolled-peer limit of {MAX_ENROLLED_PEERS} reached");
        }

        let mut next = current.clone();
        next.candidates.remove(agent_id);
        next.enrolled.insert(
            agent_id.clone(),
            EnrolledPeer {
                identity: candidate.identity.clone(),
                locators: BTreeSet::new(),
                observations: candidate.observations,
            },
        );
        self.store.save(next.stored_peers()).await?;
        self.commit(&mut current, next);
        Ok(candidate.identity)
    }

    pub async fn enroll(
        &self,
        identity: PeerIdentity,
        locators: Vec<PeerLocator>,
    ) -> Result<PeerIdentity> {
        if identity.agent_id() == &self.local_agent_id {
            bail!("cannot enroll the local Agent ID");
        }
        let mut current = self.state.write().await;
        if !current.enrolled.contains_key(identity.agent_id())
            && current.enrolled.len() >= MAX_ENROLLED_PEERS
        {
            bail!("enrolled-peer limit of {MAX_ENROLLED_PEERS} reached");
        }

        let mut next = current.clone();
        let candidate = next.candidates.remove(identity.agent_id());
        let peer = next
            .enrolled
            .entry(identity.agent_id().clone())
            .or_insert_with(|| EnrolledPeer {
                identity: identity.clone(),
                locators: BTreeSet::new(),
                observations: candidate
                    .map(|candidate| candidate.observations)
                    .unwrap_or_default(),
            });
        peer.locators.extend(locators);
        // The post-extend bound is the single authority: it validates the
        // peer's final locator set, whether the input came from one call or
        // accumulated across calls.
        if peer.locators.len() > MAX_LOCATORS_PER_PEER {
            bail!("a peer may have at most {MAX_LOCATORS_PER_PEER} configured locators");
        }
        self.store.save(next.stored_peers()).await?;
        self.commit(&mut current, next);
        Ok(identity)
    }

    pub async fn remove_peer(&self, agent_id: &AgentId) -> Result<PeerIdentity> {
        let mut current = self.state.write().await;
        let mut next = current.clone();
        let removed = next
            .enrolled
            .remove(agent_id)
            .ok_or_else(|| anyhow::anyhow!("peer {agent_id} is not enrolled"))?;
        for observation_id in removed.observations.keys() {
            next.observation_index.remove(observation_id);
        }
        self.store.save(next.stored_peers()).await?;
        self.commit(&mut current, next);
        Ok(removed.identity)
    }

    pub async fn get_enrolled(&self, agent_id: &AgentId) -> Option<PeerIdentity> {
        self.state
            .read()
            .await
            .enrolled
            .get(agent_id)
            .map(|peer| peer.identity.clone())
    }

    pub async fn enrolled_agent_ids(&self) -> Vec<AgentId> {
        self.state.read().await.enrolled.keys().cloned().collect()
    }

    pub async fn dial_targets(&self, agent_id: &AgentId) -> Vec<DialTarget> {
        let state = self.state.read().await;
        let Some(peer) = state.enrolled.get(agent_id) else {
            return Vec::new();
        };
        let mut targets: Vec<_> = peer
            .locators
            .iter()
            .cloned()
            .map(DialTarget::Configured)
            .collect();
        targets.extend(peer.observations.values().filter_map(|observation| {
            (!observation.conflicted)
                .then_some(observation.endpoint)
                .flatten()
                .map(DialTarget::Observed)
        }));
        targets
    }

    pub async fn list(&self) -> Vec<PeerView> {
        self.state.read().await.views()
    }

    fn commit(&self, current: &mut DirectoryState, next: DirectoryState) {
        let pins = next.pinning_snapshot();
        *current = next;
        match self.pins.write() {
            Ok(mut guard) => *guard = pins,
            Err(poisoned) => *poisoned.into_inner() = pins,
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "properties.rs"]
mod properties;

#[cfg(test)]
#[path = "state_machine.rs"]
mod state_machine_tests;
