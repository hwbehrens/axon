//! Opt-in integration coverage for the host's local-link Bonjour boundary.
//!
//! This test is intentionally ignored by default: multicast DNS depends on the
//! host network configuration and is not a deterministic CI primitive.

use std::net::UdpSocket;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use axon::config::AxonPaths;
use axon::discovery::{DiscoveryEvent, run_mdns_discovery};
use axon::identity::Identity;
use axon::message::AgentId;
use axon::peer_directory::{ObserveOutcome, PeerDirectory, PeerObservation, PeerStore, PeerTrust};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

fn free_port() -> u16 {
    UdpSocket::bind(("127.0.0.1", 0))
        .expect("the test host must provide an ephemeral UDP port")
        .local_addr()
        .expect("the ephemeral UDP socket must expose its local address")
        .port()
}

async fn wait_for_observation(
    receiver: &mut mpsc::Receiver<DiscoveryEvent>,
    expected: &AgentId,
) -> Result<PeerObservation> {
    let deadline = tokio::time::Instant::now() + OBSERVATION_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event = tokio::time::timeout(remaining, receiver.recv())
            .await
            .context("timed out waiting for a Bonjour observation")?
            .context("Bonjour discovery task closed before observing the peer")?;
        if let DiscoveryEvent::Observed(observation) = event
            && observation.identity.agent_id() == expected
        {
            return Ok(observation);
        }
    }
}

async fn exercise_discovery(
    receiver_a: &mut mpsc::Receiver<DiscoveryEvent>,
    receiver_b: &mut mpsc::Receiver<DiscoveryEvent>,
    identity_a: &Identity,
    identity_b: &Identity,
    paths_a: &AxonPaths,
    paths_b: &AxonPaths,
) -> Result<()> {
    let agent_a = AgentId::parse(identity_a.agent_id())?;
    let agent_b = AgentId::parse(identity_b.agent_id())?;
    let observation_a = wait_for_observation(receiver_a, &agent_b).await?;
    let observation_b = wait_for_observation(receiver_b, &agent_a).await?;

    ensure!(
        observation_a.endpoint.is_some() && observation_b.endpoint.is_some(),
        "Bonjour resolution must provide a non-loopback endpoint"
    );

    let directory_a = PeerDirectory::load(agent_a.clone(), PeerStore::new(paths_a.peers.clone()))
        .await
        .context("failed to load peer directory A")?;
    let directory_b = PeerDirectory::load(agent_b.clone(), PeerStore::new(paths_b.peers.clone()))
        .await
        .context("failed to load peer directory B")?;

    ensure!(
        directory_a.observe(observation_a).await == ObserveOutcome::CandidateAdded,
        "Bonjour discovery must create an untrusted candidate on A"
    );
    ensure!(
        directory_b.observe(observation_b).await == ObserveOutcome::CandidateAdded,
        "Bonjour discovery must create an untrusted candidate on B"
    );

    for (directory, remote, label) in [(&directory_a, &agent_b, "A"), (&directory_b, &agent_a, "B")]
    {
        let peer = directory
            .list()
            .await
            .into_iter()
            .find(|peer| peer.identity.agent_id() == remote)
            .with_context(|| format!("Bonjour candidate missing from directory {label}"))?;
        ensure!(
            peer.trust == PeerTrust::Candidate,
            "Bonjour observation must not authorize trust in directory {label}"
        );
        ensure!(
            !directory
                .pinning_snapshot()
                .read()
                .map_err(|_| anyhow::anyhow!("pinning snapshot lock poisoned"))?
                .contains_key(remote.as_str()),
            "Bonjour observation must not populate the pinning snapshot in directory {label}"
        );
    }

    directory_a
        .enroll_candidate(&agent_b)
        .await
        .context("failed to explicitly enroll candidate B")?;
    directory_b
        .enroll_candidate(&agent_a)
        .await
        .context("failed to explicitly enroll candidate A")?;

    for (directory, remote, public_key, label) in [
        (&directory_a, &agent_b, identity_b.public_key_base64(), "A"),
        (&directory_b, &agent_a, identity_a.public_key_base64(), "B"),
    ] {
        ensure!(
            directory
                .pinning_snapshot()
                .read()
                .map_err(|_| anyhow::anyhow!("pinning snapshot lock poisoned"))?
                .get(remote.as_str())
                .is_some_and(|pinned_key| pinned_key == public_key),
            "explicit enrollment must publish the observed public key in directory {label}'s pinning snapshot"
        );
    }
    Ok(())
}

async fn stop_discovery_task(task: JoinHandle<Result<()>>, label: &str) -> Result<()> {
    tokio::time::timeout(SHUTDOWN_TIMEOUT, task)
        .await
        .with_context(|| format!("timed out stopping Bonjour discovery task {label}"))?
        .with_context(|| format!("Bonjour discovery task {label} panicked"))?
        .with_context(|| format!("Bonjour discovery task {label} failed"))
}

#[tokio::test]
#[ignore = "requires a usable local-link multicast interface and Bonjour/mDNS"]
async fn live_bonjour_discovers_candidates_before_explicit_enrollment() {
    let root_a = TempDir::new().expect("temporary state root A");
    let root_b = TempDir::new().expect("temporary state root B");
    let paths_a = AxonPaths::from_root(root_a.path().to_path_buf());
    let paths_b = AxonPaths::from_root(root_b.path().to_path_buf());
    let identity_a = Identity::load_or_generate(&paths_a).expect("identity A");
    let identity_b = Identity::load_or_generate(&paths_b).expect("identity B");
    let agent_a = AgentId::parse(identity_a.agent_id()).expect("Agent ID A");
    let agent_b = AgentId::parse(identity_b.agent_id()).expect("Agent ID B");

    let (sender_a, mut receiver_a) = mpsc::channel(64);
    let (sender_b, mut receiver_b) = mpsc::channel(64);
    let cancel_a = CancellationToken::new();
    let cancel_b = CancellationToken::new();
    let task_a = tokio::spawn(run_mdns_discovery(
        agent_a,
        identity_a.public_key_base64().to_owned(),
        free_port(),
        sender_a,
        cancel_a.clone(),
    ));
    let task_b = tokio::spawn(run_mdns_discovery(
        agent_b,
        identity_b.public_key_base64().to_owned(),
        free_port(),
        sender_b,
        cancel_b.clone(),
    ));

    let exercise_result = exercise_discovery(
        &mut receiver_a,
        &mut receiver_b,
        &identity_a,
        &identity_b,
        &paths_a,
        &paths_b,
    )
    .await;

    cancel_a.cancel();
    cancel_b.cancel();
    let stop_a = stop_discovery_task(task_a, "A").await;
    let stop_b = stop_discovery_task(task_b, "B").await;

    stop_a.expect("Bonjour discovery task A must stop cleanly");
    stop_b.expect("Bonjour discovery task B must stop cleanly");
    exercise_result.expect("live Bonjour discovery must preserve candidate/enrollment semantics");
}
