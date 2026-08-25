use std::collections::{BTreeMap, BTreeSet};

use hegel::{TestCase, generators as gs, stateful};
use tempfile::TempDir;

use super::properties::{LOCAL_SEED, prop_identity};
use super::*;

/// Hegel stateful test for [`PeerDirectory`].
///
/// Rules apply discovery and enrollment actions; invariants are checked by
/// the engine after every rule application. When an invariant breaks,
/// Hegel shrinks to a minimal rule sequence, which is why this replaces
/// the previous hand-rolled proptest op list.
struct DirectoryMachine {
    rt: tokio::runtime::Runtime,
    root: TempDir,
    directory: PeerDirectory,
    store: PeerStore,
    enrolled: BTreeMap<u64, PeerIdentity>,
    candidates: BTreeMap<u64, PeerIdentity>,
    /// Observation ID -> (owning peer seed, claimed endpoint port).
    live: BTreeMap<String, (u64, u16)>,
    locators: BTreeMap<u64, BTreeSet<String>>,
}

impl DirectoryMachine {
    fn new() -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio current-thread runtime");
        let root = tempfile::tempdir().expect("tempdir");
        let store = PeerStore::new(root.path().join("peers.json"));
        let directory = rt
            .block_on(PeerDirectory::load(
                prop_identity(LOCAL_SEED).agent_id().clone(),
                PeerStore::new(root.path().join("peers.json")),
            ))
            .expect("load empty directory");
        Self {
            rt,
            root,
            directory,
            store,
            enrolled: BTreeMap::new(),
            candidates: BTreeMap::new(),
            live: BTreeMap::new(),
            locators: BTreeMap::new(),
        }
    }

    fn observation_id(slot: u64) -> String {
        format!("hegel-mdns-{slot}")
    }

    fn conflicted_ports(&self) -> BTreeSet<u16> {
        let mut owners: BTreeMap<u16, BTreeSet<u64>> = BTreeMap::new();
        for (peer, port) in self.live.values() {
            owners.entry(*port).or_default().insert(*peer);
        }
        owners
            .into_iter()
            .filter(|(_, peers)| peers.len() > 1)
            .map(|(port, _)| port)
            .collect()
    }

    fn prune_candidate(&mut self, peer: u64) {
        let still_live = self.live.values().any(|(owner, _)| *owner == peer);
        if !self.enrolled.contains_key(&peer) && !still_live {
            self.candidates.remove(&peer);
        }
    }

    /// Ports currently claimed by two or more identities.
    fn pin_map(&self) -> BTreeMap<String, String> {
        self.enrolled
            .values()
            .map(|identity| {
                (
                    identity.agent_id().as_str().to_string(),
                    identity.public_key().to_string(),
                )
            })
            .collect()
    }

    fn observation_for(&self, peer: u64, slot: u64, endpoint_port: u16) -> PeerObservation {
        let identity = prop_identity(peer);
        PeerObservation::new(
            ObservationId::new(Self::observation_id(slot)).expect("valid observation id"),
            identity.agent_id().clone(),
            identity.public_key(),
            Some(format!("127.0.0.1:{endpoint_port}").parse().unwrap()),
            None,
            ObservationSource::Mdns,
        )
        .expect("valid observation")
    }
}

#[hegel::state_machine]
impl DirectoryMachine {
    #[rule]
    fn observe(&mut self, tc: TestCase) {
        let peer = tc.draw(gs::integers::<u64>().max_value(5));
        // Slot range crosses MAX_OBSERVATIONS_PER_PEER so the per-peer
        // observation capacity limit is exercised, not just admired.
        let slot = tc.draw(gs::integers::<u64>().max_value(19));
        let port = 7000u16 + tc.draw(gs::integers::<u16>().max_value(31));
        let id = Self::observation_id(slot);

        // An observation ID claimed by a different identity is rejected
        // outright, before any mutation: discovery instance IDs are
        // globally unique per service instance.
        if self.live.get(&id).is_some_and(|(owner, _)| *owner != peer) {
            let outcome = self.rt.block_on(
                self.directory
                    .observe(self.observation_for(peer, slot, port)),
            );
            assert_eq!(
                outcome,
                ObserveOutcome::IdentityConflict,
                "cross-identity reuse of an observation ID must be rejected"
            );
            return;
        }

        // The directory withdraws the owner's previous claim first, possibly
        // retiring a candidate that just lost its last observation, before
        // classifying add/refresh.
        self.live.remove(&id);
        let was_enrolled = self.enrolled.contains_key(&peer);
        let was_candidate = self.candidates.contains_key(&peer);
        self.live.insert(id.clone(), (peer, port));

        let outcome = self.rt.block_on(
            self.directory
                .observe(self.observation_for(peer, slot, port)),
        );
        if !was_enrolled {
            self.candidates.insert(peer, prop_identity(peer));
        }
        let conflicted = self.conflicted_ports().contains(&port);
        let expected = if conflicted {
            ObserveOutcome::LocatorConflict
        } else if was_enrolled {
            ObserveOutcome::EnrolledPeerRefreshed
        } else if was_candidate {
            ObserveOutcome::CandidateRefreshed
        } else {
            ObserveOutcome::CandidateAdded
        };
        assert_eq!(
            outcome, expected,
            "unexpected observe outcome for peer {peer} on slot {slot}"
        );
        assert_ne!(
            outcome,
            ObserveOutcome::IdentityConflict,
            "validated observations carry self-derived identities, so \
             well-formed input cannot register an identity conflict"
        );
        assert_ne!(
            outcome,
            ObserveOutcome::IgnoredSelf,
            "generated seeds never collide with the local identity"
        );
        assert_ne!(
            outcome,
            ObserveOutcome::CapacityReached,
            "the generator stays within candidate and observation bounds"
        );
    }

    #[rule]
    fn drop_observation(&mut self, tc: TestCase) {
        let slot = tc.draw(gs::integers::<u64>().max_value(11));
        let id = Self::observation_id(slot);
        if let Some((owner, _)) = self.live.remove(&id) {
            self.rt.block_on(
                self.directory
                    .remove_observation(&ObservationId::new(id).expect("valid id")),
            );
            self.prune_candidate(owner);
        }
    }

    #[rule]
    fn enroll_candidate(&mut self, tc: TestCase) {
        let peer = tc.draw(gs::integers::<u64>().max_value(5));
        let result = self.rt.block_on(
            self.directory
                .enroll_candidate(prop_identity(peer).agent_id()),
        );
        if self.candidates.contains_key(&peer) {
            let identity = result.expect("candidate enrollment succeeds");
            assert_eq!(identity, prop_identity(peer));
            self.enrolled
                .insert(peer, self.candidates.remove(&peer).unwrap());
        } else if self.enrolled.contains_key(&peer) {
            // Enrollment is idempotent for an already-enrolled peer.
            let identity = result.expect("re-enrolling an enrolled peer succeeds");
            assert_eq!(identity, prop_identity(peer));
        } else {
            assert!(
                result.is_err(),
                "enrolling an unobserved candidate must fail"
            );
        }
    }

    #[rule]
    fn enroll_with_locators(&mut self, tc: TestCase) {
        let peer = tc.draw(gs::integers::<u64>().max_value(5));
        let locator_count = tc.draw(gs::integers::<u64>().max_value(2)) as usize;
        let mut incoming = BTreeSet::new();
        for _index in 0..locator_count {
            let seed = tc.draw(gs::integers::<u64>().max_value(3));
            incoming.insert(format!("svc-{seed}.internal:{}", 7000 + seed));
        }

        let mut proposed = self.locators.get(&peer).cloned().unwrap_or_default();
        proposed.extend(incoming);

        let locators: Vec<PeerLocator> = proposed
            .iter()
            .map(|raw| PeerLocator::parse(raw).expect("valid locator"))
            .collect();
        let result = self
            .rt
            .block_on(self.directory.enroll(prop_identity(peer), locators));
        if proposed.len() > MAX_LOCATORS_PER_PEER {
            assert!(
                result.is_err(),
                "locator bound must reject the enrollment for peer {peer}"
            );
        } else {
            result.expect("enrollment within bounds succeeds");
            self.enrolled.insert(peer, prop_identity(peer));
            self.candidates.remove(&peer);
            self.locators.insert(peer, proposed);
        }
    }

    #[rule]
    fn revoke(&mut self, tc: TestCase) {
        let peer = tc.draw(gs::integers::<u64>().max_value(5));
        let was_enrolled = self.enrolled.remove(&peer).is_some();
        self.locators.remove(&peer);
        let result = self
            .rt
            .block_on(self.directory.remove_peer(prop_identity(peer).agent_id()));
        if was_enrolled {
            result.expect("revoking an enrolled peer succeeds");
            self.live.retain(|_, (owner, _)| *owner != peer);
        } else {
            assert!(result.is_err(), "revoking an unknown peer must fail");
        }
    }

    /// The TLS pin set is exactly the enrolled set — never candidates,
    /// never revoked identities.
    #[invariant]
    fn pin_set_equals_enrolled_trust(&self, _tc: TestCase) {
        let pins = self.directory.pinning_snapshot();
        let pins = pins.read().expect("pin snapshot lock").as_ref().clone();
        assert_eq!(
            pins,
            self.pin_map(),
            "pin set diverged from enrolled trust bindings"
        );
    }

    /// Conflicted endpoints are undialable and hidden from views while the
    /// conflicting identities survive — and quarantines RELEASE when the
    /// conflicting claim disappears. Safety alone misses release bugs: a
    /// missing conflict recompute left survivors undialable forever.
    #[invariant]
    fn conflicted_endpoints_are_quarantined_not_evicting(&self, _tc: TestCase) {
        let conflicted = self.conflicted_ports();
        for view in self.rt.block_on(self.directory.list()) {
            for addr in &view.observed_endpoints {
                assert!(
                    !conflicted.contains(&addr.port()),
                    "view must hide conflicted endpoint {addr}"
                );
            }
        }
        for seed in self.enrolled.keys() {
            // Every live, unconflicted endpoint this enrolled peer observed
            // must be dialable: quarantine is per-endpoint, never per-peer.
            let expected: BTreeSet<u16> = self
                .live
                .values()
                .filter(|(owner, port)| *owner == *seed && !conflicted.contains(port))
                .map(|(_, port)| *port)
                .collect();
            let targets = self
                .rt
                .block_on(self.directory.dial_targets(prop_identity(*seed).agent_id()));
            let observed_targets: BTreeSet<u16> = targets
                .iter()
                .filter_map(|target| match target {
                    DialTarget::Observed(addr) => Some(addr.port()),
                    DialTarget::Configured(_) => None,
                })
                .collect();
            for port in &expected {
                assert!(
                    observed_targets.contains(port),
                    "unconflicted endpoint {port} of {} must be dialable; got {observed_targets:?}",
                    prop_identity(*seed).agent_id()
                );
            }
            for target in &targets {
                if let DialTarget::Observed(addr) = target {
                    assert!(
                        !conflicted.contains(&addr.port()),
                        "conflicted endpoint {addr} must not be a dial target"
                    );
                }
            }
        }
    }

    /// Durable intent always equals the live enrolled set.
    #[invariant]
    fn durable_intent_matches_live_authority(&self, _tc: TestCase) {
        let stored: BTreeMap<String, String> = self
            .rt
            .block_on(self.store.load())
            .expect("durable store stays readable")
            .into_iter()
            .map(|peer| (peer.agent_id.as_str().to_string(), peer.public_key))
            .collect();
        assert_eq!(
            stored,
            self.pin_map(),
            "durable peer intent diverged from the live authority"
        );
    }

    /// A fresh process restores the identical pin set from disk.
    #[invariant]
    fn restart_restores_durable_pins(&self, _tc: TestCase) {
        let reloaded = self
            .rt
            .block_on(PeerDirectory::load(
                prop_identity(LOCAL_SEED).agent_id().clone(),
                PeerStore::new(self.root.path().join("peers.json")),
            ))
            .expect("reload persisted directory");
        let reloaded_pins = reloaded.pinning_snapshot();
        let restored = reloaded_pins.read().expect("pin snapshot lock").clone();
        assert_eq!(
            *restored,
            self.pin_map(),
            "restart did not restore durable TLS pins"
        );
    }
}

/// 20 cases x up to 50 steps keeps the in-crate suite fast while still
/// exploring thousands of rule sequences; `HEGEL_TEST_CASES` scales it
/// without code changes for deeper local or nightly runs.
#[hegel::test(test_cases = 80)]
fn directory_state_machine_preserves_trust_invariants(tc: TestCase) {
    stateful::run(DirectoryMachine::new(), tc);
}
