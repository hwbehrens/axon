use std::time::Instant;

use axon::peer_directory::{
    ObservationId, ObservationSource, PeerDirectory, PeerObservation, PeerStore, PeerTrust,
};

use crate::*;

#[tokio::test]
async fn discovery_requires_explicit_enrollment_before_pinning() {
    let (local, local_dir) = make_identity();
    let (remote, _remote_dir) = make_identity();
    let local_id = AgentId::parse(local.agent_id()).unwrap();
    let remote_id = AgentId::parse(remote.agent_id()).unwrap();
    let directory = PeerDirectory::load(
        local_id,
        PeerStore::new(local_dir.path().join("peers.json")),
    )
    .await
    .unwrap();
    let mut observation = PeerObservation::new(
        ObservationId::new("integration-mdns").unwrap(),
        remote_id.clone(),
        remote.public_key_base64(),
        Some("127.0.0.1:7100".parse().unwrap()),
        Some("remote".into()),
        ObservationSource::Mdns,
    )
    .unwrap();
    observation.observed_at = Instant::now();
    directory.observe(observation).await;

    assert_eq!(directory.list().await[0].trust, PeerTrust::Candidate);
    assert!(
        !directory
            .pinning_snapshot()
            .read()
            .unwrap()
            .contains_key(remote_id.as_str())
    );
    directory.enroll_candidate(&remote_id).await.unwrap();
    assert!(
        directory
            .pinning_snapshot()
            .read()
            .unwrap()
            .contains_key(remote_id.as_str())
    );
}
