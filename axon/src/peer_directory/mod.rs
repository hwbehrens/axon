mod state;
mod store;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::RwLock;
use tracing::debug;

use crate::message::AgentId;
use state::{CandidatePeer, DirectoryState, EnrolledPeer, LiveObservation};

pub use store::{MAX_PEER_STORE_BYTES, PeerStore, StoredPeer};
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

/// Maximum attempts for a persistent edit that keeps losing the
/// persist-generation race before the store is healed from live memory and
/// an error is returned.
const PERSIST_COMMIT_ATTEMPTS: usize = 8;

/// A validated persistent edit, built against a read snapshot of directory
/// state.
///
/// Peer-store disk I/O must not run under the state write lock: a stalled
/// save would block every reader (`dial_targets`, `is_enrolled`) — including
/// the transport's send path and connection-admission gate — indefinitely.
/// Instead an edit is validated against a snapshot, its bytes are saved with
/// no lock held, and then a short write lock applies the same delta onto
/// fresh live state (never a whole-snapshot swap, which would clobber
/// concurrent ephemeral changes such as new observations).
struct PersistPlan<T> {
    /// Snapshot whose persistent content (`stored_peers`) equals post-apply
    /// state when the persist-generation check passes. This is what gets
    /// serialized.
    saved_state: DirectoryState,
    /// Applies this edit's delta onto current live state under the short
    /// commit lock.
    apply: Box<dyn FnOnce(&mut DirectoryState) + Send>,
    /// Value returned to the caller on successful commit (or on validation
    /// fast paths such as re-enrolling an already-enrolled peer).
    value: T,
}

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
        let agent_id = agent_id.clone();
        self.commit_persistent(move |current| {
            if let Some(peer) = current.enrolled.get(&agent_id) {
                return Ok(PersistPlan {
                    saved_state: current.clone(),
                    apply: Box::new(|_| {}),
                    value: peer.identity.clone(),
                });
            }
            let candidate = current.candidates.get(&agent_id).cloned().ok_or_else(|| {
                anyhow::anyhow!("peer candidate {agent_id} is not currently observed")
            })?;
            if current.enrolled.len() >= MAX_ENROLLED_PEERS {
                bail!("enrolled-peer limit of {MAX_ENROLLED_PEERS} reached");
            }

            let identity = candidate.identity.clone();
            let mut next = current.clone();
            next.candidates.remove(&agent_id);
            next.enrolled.insert(
                agent_id.clone(),
                EnrolledPeer {
                    identity: identity.clone(),
                    locators: BTreeSet::new(),
                    observations: candidate.observations,
                },
            );
            let promote_agent_id = agent_id.clone();
            let promote_identity = identity.clone();
            Ok(PersistPlan {
                saved_state: next,
                apply: Box::new(move |state| {
                    // The candidate's CURRENT observation set is taken at
                    // apply time: ephemeral refreshes between snapshot and
                    // commit are legitimate and move with the promotion.
                    let observations = state
                        .candidates
                        .remove(&promote_agent_id)
                        .map(|candidate| candidate.observations)
                        .unwrap_or_default();
                    state.enrolled.insert(
                        promote_agent_id,
                        EnrolledPeer {
                            identity: promote_identity,
                            locators: BTreeSet::new(),
                            observations,
                        },
                    );
                }),
                value: identity,
            })
        })
        .await
    }

    pub async fn enroll(
        &self,
        identity: PeerIdentity,
        locators: Vec<PeerLocator>,
    ) -> Result<PeerIdentity> {
        if identity.agent_id() == &self.local_agent_id {
            bail!("cannot enroll the local Agent ID");
        }
        let agent_id = identity.agent_id().clone();
        self.commit_persistent(move |current| {
            let identity = identity.clone();
            if !current.enrolled.contains_key(identity.agent_id())
                && current.enrolled.len() >= MAX_ENROLLED_PEERS
            {
                bail!("enrolled-peer limit of {MAX_ENROLLED_PEERS} reached");
            }

            let mut next = current.clone();
            next.candidates.remove(&agent_id);
            let peer = next
                .enrolled
                .entry(agent_id.clone())
                .or_insert_with(|| EnrolledPeer {
                    identity: identity.clone(),
                    locators: BTreeSet::new(),
                    observations: BTreeMap::new(),
                });
            peer.locators.extend(locators.iter().cloned());
            // The post-extend bound is the single authority: it validates the
            // peer's final locator set, whether the input came from one call or
            // accumulated across calls.
            if peer.locators.len() > MAX_LOCATORS_PER_PEER {
                bail!("a peer may have at most {MAX_LOCATORS_PER_PEER} configured locators");
            }

            // Per-attempt clones: `build` is an `Fn` (the retry loop may
            // invoke it repeatedly), so it cannot move its captures.
            let (apply_agent_id, apply_identity, apply_locators) =
                (agent_id.clone(), identity.clone(), locators.clone());

            Ok(PersistPlan {
                saved_state: next,
                apply: Box::new(move |state| {
                    let observations = state
                        .candidates
                        .remove(&apply_agent_id)
                        .map(|candidate| candidate.observations)
                        .unwrap_or_default();
                    let peer =
                        state
                            .enrolled
                            .entry(apply_agent_id)
                            .or_insert_with(|| EnrolledPeer {
                                identity: apply_identity,
                                locators: BTreeSet::new(),
                                observations,
                            });
                    peer.locators.extend(apply_locators);
                }),
                value: identity,
            })
        })
        .await
    }

    pub async fn remove_peer(&self, agent_id: &AgentId) -> Result<PeerIdentity> {
        let agent_id = agent_id.clone();
        self.commit_persistent(move |current| {
            let removed = current
                .enrolled
                .get(&agent_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("peer {agent_id} is not enrolled"))?;
            let mut next = current.clone();
            next.enrolled.remove(&agent_id);
            for observation_id in removed.observations.keys() {
                next.observation_index.remove(observation_id);
            }
            // The revoked peer's endpoint claims die with it; survivors that were
            // quarantined against those claims must become dialable again.
            next.recompute_conflicts();

            let revoke_agent_id = agent_id.clone();
            let revoked_observation_ids: Vec<_> = removed.observations.keys().cloned().collect();
            Ok(PersistPlan {
                saved_state: next,
                apply: Box::new(move |state| {
                    state.enrolled.remove(&revoke_agent_id);
                    for id in revoked_observation_ids {
                        state.observation_index.remove(&id);
                    }
                    // Anything observed AFTER the revocation snapshot is
                    // legitimate post-revocation discovery and stays: only
                    // the enrollment and the observations that existed at
                    // snapshot time are removed here.
                    state.recompute_conflicts();
                }),
                value: removed.identity,
            })
        })
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

    /// Commit a persistent edit without holding the directory write lock
    /// across peer-store disk I/O.
    ///
    /// Protocol per attempt: build and validate [`PersistPlan`] under a read
    /// lock, save its bytes with NO lock held, then take the write lock and
    /// apply the delta only if no other persistent edit landed in between
    /// (`persist_generation`). On a lost race the whole edit is retried
    /// against fresh state; after too many races the store is healed from
    /// live memory and an error is surfaced rather than committing blind.
    async fn commit_persistent<T>(
        &self,
        build: impl Fn(&DirectoryState) -> Result<PersistPlan<T>>,
    ) -> Result<T> {
        for attempt in 0..PERSIST_COMMIT_ATTEMPTS {
            let (plan, generation) = {
                let state = self.state.read().await;
                (build(&state)?, state.persist_generation)
            };
            self.store.save(plan.saved_state.stored_peers()).await?;
            let mut state = self.state.write().await;
            if state.persist_generation != generation {
                debug!(attempt, "peer-directory edit lost a persist race; retrying");
                continue;
            }
            (plan.apply)(&mut state);
            state.persist_generation += 1;
            let pins = state.pinning_snapshot();
            drop(state);
            match self.pins.write() {
                Ok(mut guard) => *guard = pins,
                Err(poisoned) => *poisoned.into_inner() = pins,
            }
            return Ok(plan.value);
        }
        // Heal any divergence the last speculative save left behind: memory
        // is authoritative, so re-serialize it before giving up.
        let state = self.state.read().await;
        self.store.save(state.stored_peers()).await?;
        bail!(
            "peer directory changed concurrently during persistence; \
             edit abandoned after {PERSIST_COMMIT_ATTEMPTS} attempts"
        )
    }
}

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
#[path = "properties.rs"]
mod properties;

#[cfg(test)]
#[path = "state_machine.rs"]
mod state_machine_tests;
