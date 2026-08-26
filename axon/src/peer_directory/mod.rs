mod state;
mod store;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::{Mutex as SaveMutex, RwLock};

use crate::message::AgentId;
use state::{CandidatePeer, DirectoryState, EnrolledPeer, LiveObservation};

pub use store::{MAX_PEER_STORE_BYTES, PeerStore, StoredPeer};
pub use types::{
    DialTarget, DirectoryError, ObservationId, ObservationSource, ObserveOutcome, PeerIdentity,
    PeerLocator, PeerObservation, PeerTrust, PeerView,
};

use self::persistence::PersistPlan;

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
    /// Serializes peer-store writes (see `persistence`); never held while
    /// the directory state lock is held.
    save_lock: Arc<SaveMutex<()>>,
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
            save_lock: Arc::new(SaveMutex::new(())),
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

    pub async fn enroll_candidate(
        &self,
        agent_id: &AgentId,
    ) -> Result<PeerIdentity, DirectoryError> {
        let agent_id = agent_id.clone();
        self.commit_persistent(move |current| enroll_candidate_plan(current, &agent_id))
            .await
    }

    #[cfg(test)]
    pub(super) fn enroll_candidate_detached(
        &self,
        agent_id: &AgentId,
    ) -> tokio::task::JoinHandle<Result<PeerIdentity, DirectoryError>> {
        let agent_id = agent_id.clone();
        self.spawn_persistent_edit(move |current| enroll_candidate_plan(current, &agent_id))
    }

    pub async fn enroll(
        &self,
        identity: PeerIdentity,
        locators: Vec<PeerLocator>,
    ) -> Result<PeerIdentity, DirectoryError> {
        if identity.agent_id() == &self.local_agent_id {
            return Err(DirectoryError::LocalAgentId(identity.agent_id().clone()));
        }
        self.commit_persistent(move |current| {
            // Per-attempt clones: `build` is an `Fn` (the retry loop may
            // invoke it repeatedly), so it cannot move its captures.
            let identity = identity.clone();
            enroll_plan(current, &identity, &locators)
        })
        .await
    }

    pub async fn remove_peer(&self, agent_id: &AgentId) -> Result<PeerIdentity, DirectoryError> {
        let agent_id = agent_id.clone();
        self.commit_persistent(move |current| remove_peer_plan(current, &agent_id))
            .await
    }

    pub async fn get_enrolled(&self, agent_id: &AgentId) -> Option<PeerIdentity> {
        self.state
            .read()
            .await
            .enrolled
            .get(agent_id)
            .map(|peer| peer.identity.clone())
    }

    /// Enrollment predicate used as the transport's connection-admission
    /// gate. It shares the directory state lock with `remove_peer`'s short
    /// commit section, so a result observed under the registry's admission
    /// lock is linearized against revocation (see
    /// `ConnectionRegistry::admit_gated`). Persistence runs outside this
    /// lock, so the gate can never be stalled by peer-store disk I/O.
    pub async fn is_enrolled(&self, agent_id: &AgentId) -> bool {
        self.state.read().await.enrolled.contains_key(agent_id)
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
}

/// Promotion plan: candidate -> enrolled, keeping the candidate's CURRENT
/// observation set (refreshes between snapshot and commit move with the
/// promotion).
fn enroll_candidate_plan(
    current: &DirectoryState,
    agent_id: &AgentId,
) -> Result<PersistPlan<PeerIdentity>, DirectoryError> {
    if let Some(peer) = current.enrolled.get(agent_id) {
        return Ok(PersistPlan {
            saved_state: current.clone(),
            apply: Box::new(|_| {}),
            value: peer.identity.clone(),
        });
    }
    let candidate = current
        .candidates
        .get(agent_id)
        .cloned()
        .ok_or_else(|| DirectoryError::NotObserved(agent_id.clone()))?;
    if current.enrolled.len() >= MAX_ENROLLED_PEERS {
        return Err(DirectoryError::EnrolledCapacity);
    }

    let identity = candidate.identity.clone();
    let mut next = current.clone();
    next.candidates.remove(agent_id);
    next.enrolled.insert(
        agent_id.clone(),
        EnrolledPeer {
            identity: identity.clone(),
            locators: BTreeSet::new(),
            observations: candidate.observations,
        },
    );
    let (apply_agent_id, apply_identity) = (agent_id.clone(), identity.clone());
    Ok(PersistPlan {
        saved_state: next,
        apply: Box::new(move |state| {
            let observations = state
                .candidates
                .remove(&apply_agent_id)
                .map(|candidate| candidate.observations)
                .unwrap_or_default();
            state.enrolled.insert(
                apply_agent_id,
                EnrolledPeer {
                    identity: apply_identity,
                    locators: BTreeSet::new(),
                    observations,
                },
            );
        }),
        value: identity,
    })
}

/// Upsert plan for explicit enrollment with configured locators.
fn enroll_plan(
    current: &DirectoryState,
    identity: &PeerIdentity,
    locators: &[PeerLocator],
) -> Result<PersistPlan<PeerIdentity>, DirectoryError> {
    let agent_id = identity.agent_id();
    if !current.enrolled.contains_key(agent_id) && current.enrolled.len() >= MAX_ENROLLED_PEERS {
        return Err(DirectoryError::EnrolledCapacity);
    }

    let mut next = current.clone();
    next.candidates.remove(agent_id);
    let peer = next
        .enrolled
        .entry(agent_id.clone())
        .or_insert_with(|| EnrolledPeer {
            identity: identity.clone(),
            locators: BTreeSet::new(),
            observations: BTreeMap::new(),
        });
    peer.locators.extend(locators.iter().cloned());
    // The post-extend bound is the single authority: it validates the peer's
    // final locator set, whether the input came from one call or accumulated
    // across calls.
    if peer.locators.len() > MAX_LOCATORS_PER_PEER {
        return Err(DirectoryError::LocatorCapacity);
    }

    let (apply_agent_id, apply_identity, apply_locators) =
        (agent_id.clone(), identity.clone(), locators.to_vec());
    Ok(PersistPlan {
        saved_state: next,
        apply: Box::new(move |state| {
            let observations = state
                .candidates
                .remove(&apply_agent_id)
                .map(|candidate| candidate.observations)
                .unwrap_or_default();
            let peer = state
                .enrolled
                .entry(apply_agent_id)
                .or_insert_with(|| EnrolledPeer {
                    identity: apply_identity,
                    locators: BTreeSet::new(),
                    observations,
                });
            peer.locators.extend(apply_locators);
        }),
        value: identity.clone(),
    })
}

/// Revocation plan. The apply step removes the record's ENTIRE observation
/// set at commit time — the snapshot IDs plus any observation a concurrent
/// `observe` landed on the still-enrolled record between snapshot and
/// commit. Cleaning only the snapshot set (the round-six behavior) orphaned
/// those raced-in IDs in `observation_index`: entries whose agent no longer
/// has a record, invisible to expiry, leaking forever. Observations that
/// arrive AFTER the commit are legitimate post-revocation discovery: they
/// create a fresh candidate.
fn remove_peer_plan(
    current: &DirectoryState,
    agent_id: &AgentId,
) -> Result<PersistPlan<PeerIdentity>, DirectoryError> {
    let removed = current
        .enrolled
        .get(agent_id)
        .cloned()
        .ok_or_else(|| DirectoryError::NotEnrolled(agent_id.clone()))?;
    let mut next = current.clone();
    next.enrolled.remove(agent_id);
    for observation_id in removed.observations.keys() {
        next.observation_index.remove(observation_id);
    }
    // The revoked peer's endpoint claims die with it; survivors that were
    // quarantined against those claims must become dialable again.
    next.recompute_conflicts();

    let apply_agent_id = agent_id.clone();
    Ok(PersistPlan {
        saved_state: next,
        apply: Box::new(move |state| {
            if let Some(record) = state.enrolled.remove(&apply_agent_id) {
                for observation_id in record.observations.keys() {
                    state.observation_index.remove(observation_id);
                }
            }
            state.recompute_conflicts();
        }),
        value: removed.identity,
    })
}

#[path = "persistence.rs"]
mod persistence;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;

#[cfg(test)]
#[path = "limits_tests.rs"]
mod limits_tests;

#[cfg(test)]
#[path = "interleaving_tests.rs"]
mod interleaving_tests;

#[cfg(test)]
#[path = "properties.rs"]
mod properties;

#[cfg(test)]
#[path = "state_machine.rs"]
mod state_machine_tests;
