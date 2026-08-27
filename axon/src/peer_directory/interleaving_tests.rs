//! Round-seven review regressions (DEC-022): revocation interleavings and
//! transactional-persistence abort/cancellation behavior.
//!
//! Split from `tests.rs` for file-length limits.

use super::tests::{directory, identity, observation};
use super::*;

// ---------------------------------------------------------------------------
// Round-seven review regressions (DEC-022)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revocation_cleans_observations_that_raced_the_snapshot() {
    let (_root, directory) = directory().await;
    let remote = identity(32);
    let agent_id = remote.agent_id().clone();

    directory
        .observe(observation(32, "mdns:raced:a", "127.0.0.1:7400"))
        .await;
    directory.enroll_candidate(&agent_id).await.expect("enroll");

    // Build the removal plan against the snapshot (only observation A
    // exists) — exactly what `remove_peer` does between taking the read
    // snapshot and committing.
    let mut state = directory.state.read().await.clone();
    let plan = remove_peer_plan(&state, &agent_id).expect("removal plan");

    // ...then a concurrent `observe` lands AFTER the snapshot: the peer is
    // still enrolled, so the new observation joins the enrolled record and
    // its index entry.
    let raced_id = ObservationId::new("mdns:raced:b").expect("id");
    state
        .enrolled
        .get_mut(&agent_id)
        .expect("still enrolled at snapshot time")
        .observations
        .insert(
            raced_id.clone(),
            LiveObservation {
                endpoint: None,
                display_name: None,
                observed_at: std::time::Instant::now(),
                conflicted: false,
            },
        );
    state
        .observation_index
        .insert(raced_id.clone(), agent_id.clone());

    (plan.apply)(&mut state);

    assert!(!state.enrolled.contains_key(&agent_id));
    assert!(
        !state
            .observation_index
            .contains_key(&ObservationId::new("mdns:raced:a").expect("id"))
    );
    assert!(
        !state.observation_index.contains_key(&raced_id),
        "an observation raced in between snapshot and commit must not \
         survive as a ghost index entry"
    );
    state.assert_no_ghost_observations();
}

#[tokio::test]
async fn concurrent_observation_during_revocation_leaves_no_ghosts() {
    for round in 0..25u8 {
        let (_root, directory) = directory().await;
        let seed = 33 + round;
        let agent_id = identity(seed).agent_id().clone();
        directory
            .enroll(identity(seed), Vec::new())
            .await
            .expect("enroll");
        directory
            .observe(observation(seed, "mdns:stress:base", "127.0.0.1:7500"))
            .await;

        // Hammer observations while revocation's save-then-apply window is
        // open: whichever way the interleaving lands, the index must stay
        // ghost-free afterwards.
        let observer = directory.clone();
        let churn = tokio::spawn(async move {
            for index in 0..64usize {
                let _ = observer
                    .observe(observation(
                        seed,
                        &format!("mdns:stress:{index}"),
                        &format!("127.0.0.1:{}", 7501 + index),
                    ))
                    .await;
            }
        });
        directory.remove_peer(&agent_id).await.expect("revoke");
        churn.await.expect("churn task");

        directory.state.read().await.assert_no_ghost_observations();
    }
}

#[tokio::test]
async fn persistent_edit_commits_without_an_awaiter() {
    let (root, directory) = directory().await;
    directory
        .observe(observation(60, "mdns:detached", "127.0.0.1:7600"))
        .await;
    let agent_id = identity(60).agent_id().clone();

    // Fire the transaction worker and DROP the join handle: the caller's
    // future is gone (timeout, shutdown race), yet save-plus-apply must
    // still complete — cancellation can never leave disk ahead of memory.
    drop(directory.enroll_candidate_detached(&agent_id));

    let mut enrolled = false;
    for _ in 0..500 {
        if directory.get_enrolled(&agent_id).await.is_some() {
            enrolled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        enrolled,
        "detached transaction worker must complete the commit"
    );

    // The detached worker persisted too, not just applied in memory.
    let reloaded = PeerDirectory::load(
        identity(99).agent_id().clone(),
        PeerStore::new(root.path().join("peers.json")),
    )
    .await
    .expect("reload");
    assert!(
        reloaded.get_enrolled(&agent_id).await.is_some(),
        "detached worker must have durably persisted the enrollment"
    );
}

#[tokio::test]
async fn save_failure_leaves_memory_and_disk_consistent() {
    let root = tempfile::tempdir().expect("tempdir");
    let store_path = root.path().join("peers.json");
    let directory = PeerDirectory::load(
        identity(99).agent_id().clone(),
        PeerStore::new(store_path.clone()),
    )
    .await
    .expect("load");
    directory
        .observe(observation(61, "mdns:failing-save", "127.0.0.1:7700"))
        .await;
    let agent_id = identity(61).agent_id().clone();

    // Failure injection: squat the store path with a directory so the
    // atomic rename inside `save` fails BEFORE any new content lands
    // (temp-file creation still succeeds; the rename over a directory
    // cannot).
    std::fs::create_dir(&store_path).expect("squat the store path");

    let result = directory.enroll_candidate(&agent_id).await;

    assert!(
        result.is_err(),
        "enrollment must fail when persistence fails"
    );
    assert!(
        directory.get_enrolled(&agent_id).await.is_none(),
        "memory must not advance past a failed save"
    );
    assert!(
        store_path.is_dir(),
        "the squatted path must be untouched: no store file replaced it"
    );
    directory.state.read().await.assert_no_ghost_observations();
}

#[tokio::test]
async fn concurrent_edits_leave_disk_equal_to_memory() {
    let (root, directory) = directory().await;
    let mut tasks = Vec::new();
    for seed in 62..70u8 {
        let directory = directory.clone();
        tasks.push(tokio::spawn(async move {
            let identity = identity(seed);
            let agent_id = identity.agent_id().clone();
            for round in 0..4u32 {
                directory
                    .enroll(
                        identity.clone(),
                        vec![
                            PeerLocator::parse(&format!("127.0.0.1:{}", 8100 + seed as u16))
                                .expect("locator"),
                        ],
                    )
                    .await
                    .expect("enroll");
                if round % 2 == 1 {
                    directory.remove_peer(&agent_id).await.expect("revoke");
                }
            }
        }));
    }
    for task in tasks {
        task.await.expect("edit task");
    }

    // After quiescence the durable store must equal live memory. The
    // transaction gate (save lock held across save PLUS generation-checked
    // apply) totally orders every save+commit pair, so no interleaving —
    // including heal racing a paused committer — can leave disk older than
    // memory.
    let mut live: Vec<String> = directory
        .enrolled_agent_ids()
        .await
        .into_iter()
        .map(|agent| agent.to_string())
        .collect();
    live.sort();
    let mut stored: Vec<String> = PeerStore::new(root.path().join("peers.json"))
        .load()
        .await
        .expect("durable store stays readable")
        .into_iter()
        .map(|peer| peer.agent_id.to_string())
        .collect();
    stored.sort();
    assert_eq!(
        live, stored,
        "durable peer set must equal the live authority after concurrent edits"
    );

    directory.state.read().await.assert_no_ghost_observations();
}
