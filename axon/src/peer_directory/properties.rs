use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

use super::*;

/// Deterministic 32-byte public key material for a seed so that every
/// reference to the same seed yields the same (Agent ID, public key) pair.
pub(super) fn key_for_seed(seed: u64) -> [u8; 32] {
    Sha256::digest(seed.to_be_bytes()).into()
}

pub(super) fn prop_identity(seed: u64) -> PeerIdentity {
    PeerIdentity::from_public_key(&STANDARD.encode(key_for_seed(seed))).expect("valid test key")
}

/// Seed reserved for the local daemon; generated remote seeds never reach it.
pub(super) const LOCAL_SEED: u64 = u64::MAX;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

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
