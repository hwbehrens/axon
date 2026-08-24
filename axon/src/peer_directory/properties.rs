use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};

use super::*;

/// Deterministic 32-byte public key material for a seed so that every
/// reference to the same seed yields the same (Agent ID, public key) pair.
fn key_for_seed(seed: u64) -> [u8; 32] {
    Sha256::digest(seed.to_be_bytes()).into()
}

fn prop_identity(seed: u64) -> PeerIdentity {
    PeerIdentity::from_public_key(&STANDARD.encode(key_for_seed(seed))).expect("valid test key")
}

/// Seed reserved for the local daemon; generated remote seeds never reach it.
const LOCAL_SEED: u64 = u64::MAX;

#[derive(Debug, Clone)]
enum DirectoryOp {
    /// Deliver an mDNS-style observation for `peer` under observation slot
    /// `slot`, claiming endpoint port `7000 + port`. Slots let later inputs
    /// refresh, move, or withdraw endpoint claims like real discovery does.
    Observe { peer: u64, slot: u64, port: u16 },
    /// Withdraw one live observation by slot, as discovery loss does.
    DropObservation(u64),
    /// Promote a currently observed candidate to enrolled trust.
    EnrollCandidate(u64),
    /// Directly enroll a peer with generated configured locators.
    Enroll { peer: u64, locator_seeds: Vec<u64> },
    /// Revoke an enrolled peer.
    Revoke(u64),
}

fn arb_op() -> impl Strategy<Value = DirectoryOp> {
    let peer = 0u64..6;
    prop_oneof![
        (peer.clone(), 0u64..12, 0u16..32).prop_map(|(peer, slot, port)| DirectoryOp::Observe {
            peer,
            slot,
            port: 7000 + port,
        }),
        (0u64..12).prop_map(DirectoryOp::DropObservation),
        peer.clone().prop_map(DirectoryOp::EnrollCandidate),
        (peer.clone(), proptest::collection::vec(0u64..4, 0..3)).prop_map(
            |(peer, locator_seeds)| DirectoryOp::Enroll {
                peer,
                locator_seeds
            }
        ),
        peer.prop_map(DirectoryOp::Revoke),
    ]
}

/// Reference model of externally observable directory facts.
///
/// It tracks only what IPC consumers can see — enrolled trust bindings,
/// candidate membership, live observation ownership, configured locators —
/// and deliberately ignores internal representation choices.
#[derive(Debug, Default)]
struct DirectoryModel {
    enrolled: BTreeMap<u64, PeerIdentity>,
    candidates: BTreeMap<u64, PeerIdentity>,
    /// Observation ID -> (owning peer seed, claimed endpoint port).
    live: BTreeMap<String, (u64, u16)>,
    locators: BTreeMap<u64, BTreeSet<String>>,
}

impl DirectoryModel {
    fn observation_id(slot: u64) -> String {
        format!("prop-mdns-{slot}")
    }

    /// Ports currently claimed by observations of two or more identities.
    fn conflicted_ports(&self) -> BTreeSet<u16> {
        let mut owners: BTreeMap<u16, BTreeSet<u64>> = BTreeMap::new();
        for (_id, (peer, port)) in &self.live {
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

    async fn apply(&mut self, directory: &PeerDirectory, op: DirectoryOp) {
        match op {
            DirectoryOp::Observe {
                peer,
                slot,
                port: endpoint_port,
            } => {
                let id = Self::observation_id(slot);
                // An observation ID claimed by a different identity is
                // rejected outright, before any mutation: discovery instance
                // IDs are globally unique per service instance.
                if self.live.get(&id).is_some_and(|(owner, _)| *owner != peer) {
                    let outcome = directory
                        .observe(observation_for(peer, slot, endpoint_port))
                        .await;
                    assert_eq!(
                        outcome,
                        ObserveOutcome::IdentityConflict,
                        "cross-identity reuse of an observation ID must be rejected"
                    );
                    return;
                }

                // The directory withdraws the owner's previous claim first,
                // possibly retiring a candidate that just lost its last
                // observation, before classifying add/refresh.
                self.live.remove(&id);
                let was_enrolled = self.enrolled.contains_key(&peer);
                let was_candidate = self.candidates.contains_key(&peer);
                self.live.insert(id.clone(), (peer, endpoint_port));

                let identity = prop_identity(peer);
                let observation = PeerObservation::new(
                    ObservationId::new(id).expect("valid observation id"),
                    identity.agent_id().clone(),
                    identity.public_key(),
                    Some(format!("127.0.0.1:{endpoint_port}").parse().unwrap()),
                    None,
                    ObservationSource::Mdns,
                )
                .expect("valid observation");
                let outcome = directory.observe(observation).await;
                if !was_enrolled {
                    self.candidates.insert(peer, prop_identity(peer));
                }

                // Conflicts are computed after the observation lands, exactly
                // as the directory recomputes post-insertion.
                let conflicted = self.conflicted_ports().contains(&endpoint_port);
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
                    ObserveOutcome::IgnoredSelf,
                    "generated seeds never collide with the local identity"
                );
                assert_ne!(
                    outcome,
                    ObserveOutcome::CapacityReached,
                    "the generator stays within candidate and observation bounds"
                );
                assert_ne!(
                    outcome,
                    ObserveOutcome::IdentityConflict,
                    "validated observations carry self-derived identities, \
                     so well-formed input cannot register an identity conflict"
                );
            }
            DirectoryOp::DropObservation(slot) => {
                let id = Self::observation_id(slot);
                if let Some((owner, _)) = self.live.remove(&id) {
                    directory
                        .remove_observation(&ObservationId::new(id).expect("valid id"))
                        .await;
                    self.prune_candidate(owner);
                }
            }
            DirectoryOp::EnrollCandidate(peer) => {
                let known_candidate = self.candidates.contains_key(&peer);
                let result = directory
                    .enroll_candidate(prop_identity(peer).agent_id())
                    .await;
                if known_candidate {
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
            DirectoryOp::Enroll {
                peer,
                locator_seeds,
            } => {
                let incoming: BTreeSet<String> = locator_seeds
                    .into_iter()
                    .map(|seed| format!("svc-{seed}.internal:{}", 7000 + seed))
                    .collect();
                let mut proposed = self.locators.get(&peer).cloned().unwrap_or_default();
                proposed.extend(incoming);

                let result = directory
                    .enroll(
                        prop_identity(peer),
                        proposed
                            .iter()
                            .map(|raw| PeerLocator::parse(raw).expect("valid locator"))
                            .collect(),
                    )
                    .await;
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
            DirectoryOp::Revoke(peer) => {
                let was_enrolled = self.enrolled.remove(&peer).is_some();
                self.locators.remove(&peer);
                let result = directory.remove_peer(prop_identity(peer).agent_id()).await;
                if was_enrolled {
                    result.expect("revoking an enrolled peer succeeds");
                    // Revocation ends the peer's live observations with it.
                    self.live.retain(|_, (owner, _)| *owner != peer);
                } else {
                    assert!(result.is_err(), "revoking an unknown peer must fail");
                }
            }
        }
    }
}

fn observation_for(peer: u64, slot: u64, endpoint_port: u16) -> PeerObservation {
    let identity = prop_identity(peer);
    PeerObservation::new(
        ObservationId::new(DirectoryModel::observation_id(slot)).expect("valid observation id"),
        identity.agent_id().clone(),
        identity.public_key(),
        Some(format!("127.0.0.1:{endpoint_port}").parse().unwrap()),
        None,
        ObservationSource::Mdns,
    )
    .expect("valid observation")
}

async fn prop_directory() -> (TempDir, PeerDirectory) {
    let root = tempdir().expect("tempdir");
    let store = PeerStore::new(root.path().join("peers.json"));
    let directory = PeerDirectory::load(prop_identity(LOCAL_SEED).agent_id().clone(), store)
        .await
        .expect("load empty directory");
    (root, directory)
}

fn pin_map(model_enrolled: &BTreeMap<u64, PeerIdentity>) -> BTreeMap<String, String> {
    model_enrolled
        .values()
        .map(|identity| {
            (
                identity.agent_id().as_str().to_string(),
                identity.public_key().to_string(),
            )
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn directory_operations_preserve_trust_and_endpoint_invariants(
        ops in proptest::collection::vec(arb_op(), 1..40),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (root, directory) = prop_directory().await;
            let mut model = DirectoryModel::default();

            for op in ops {
                model.apply(&directory, op).await;
                let expected_pins = pin_map(&model.enrolled);

                // Invariant 1: the TLS pin set is exactly the enrolled set.
                let pins = directory.pinning_snapshot();
                let current: BTreeMap<String, String> =
                    pins.read().expect("pin snapshot lock").as_ref().clone();
                prop_assert_eq!(
                    current,
                    expected_pins.clone(),
                    "pin set diverged from enrolled trust bindings"
                );

                // Invariant 2: no candidate ever appears in the pin set.
                for seed in model.candidates.keys() {
                    prop_assert!(
                        !expected_pins.contains_key(prop_identity(*seed).agent_id().as_str()),
                        "candidate {} must not hold TLS trust",
                        prop_identity(*seed).agent_id()
                    );
                }

                // Invariant 3: conflicted endpoints are undialable and hidden
                // from views, while the conflicting identities survive.
                let conflicted = model.conflicted_ports();
                for view in directory.list().await {
                    for addr in &view.observed_endpoints {
                        prop_assert!(
                            !conflicted.contains(&addr.port()),
                            "view must hide conflicted endpoint {addr}"
                        );
                    }
                }
                for seed in model.enrolled.keys() {
                    for target in directory.dial_targets(prop_identity(*seed).agent_id()).await {
                        if let DialTarget::Observed(addr) = target {
                            prop_assert!(
                                !conflicted.contains(&addr.port()),
                                "conflicted endpoint {addr} must not be a dial target"
                            );
                        }
                    }
                }
            }

            // Invariant 4: durable intent equals the live enrolled set.
            let store = PeerStore::new(root.path().join("peers.json"));
            let stored: BTreeMap<String, String> = store
                .load()
                .await
                .expect("durable store stays readable")
                .into_iter()
                .map(|peer| (peer.agent_id.as_str().to_string(), peer.public_key))
                .collect();
            prop_assert_eq!(
                stored,
                pin_map(&model.enrolled),
                "durable peer intent diverged from the live authority"
            );

            // Invariant 5: a fresh process restores the identical pin set.
            let reloaded = PeerDirectory::load(
                prop_identity(LOCAL_SEED).agent_id().clone(),
                PeerStore::new(root.path().join("peers.json")),
            )
            .await
            .expect("reload persisted directory");
            let reloaded_pins = reloaded.pinning_snapshot();
            let restored: BTreeMap<String, String> =
                reloaded_pins.read().expect("pin snapshot lock").as_ref().clone();
            prop_assert_eq!(
                restored,
                pin_map(&model.enrolled),
                "restart did not restore durable TLS pins"
            );

            Ok(())
        })?;
    }

    #[test]
    fn store_roundtrip_within_bounds_preserves_peers(
        seeds in proptest::collection::vec(0u64..256, 0..MAX_ENROLLED_PEERS),
        locator_counts in proptest::collection::vec(0usize..=MAX_LOCATORS_PER_PEER, 1..8),
    ) {
        let unique_seeds: BTreeSet<u64> = seeds.into_iter().collect();
        let peers: Vec<StoredPeer> = unique_seeds
            .iter()
            .enumerate()
            .map(|(index, seed)| {
                let identity = prop_identity(*seed);
                let count = locator_counts[index % locator_counts.len()];
                let locators: BTreeSet<PeerLocator> = (0..count)
                    .map(|i| {
                        let port = 7100 + (i % 10) as u16;
                        PeerLocator::parse(&format!("svc-{seed}-{i}.internal:{port}")).unwrap()
                    })
                    .collect();
                StoredPeer {
                    agent_id: identity.agent_id().clone(),
                    public_key: identity.public_key().to_string(),
                    locators: locators.into_iter().collect(),
                }
            })
            .collect();

        let document = serde_json::json!({ "version": 1, "peers": peers });
        let encoded = serde_json::to_vec(&document).expect("encode fixture");

        let decoded = PeerStore::decode(&encoded).expect("in-bounds store decodes");
        prop_assert_eq!(
            decoded.iter().map(|peer| peer.agent_id.clone()).collect::<Vec<_>>(),
            peers.iter().map(|peer| peer.agent_id.clone()).collect::<Vec<_>>(),
        );
        prop_assert_eq!(
            decoded.iter().map(|peer| peer.locators.clone()).collect::<Vec<_>>(),
            peers.iter().map(|peer| peer.locators.clone()).collect::<Vec<_>>(),
        );
    }
}
