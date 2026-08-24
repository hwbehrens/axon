use std::path::PathBuf;
use std::time::Duration;

use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

use super::super::{ConnectionManager, ResponseHandlerFn};
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::message::AgentId;
use crate::peer_directory::{PeerDirectory, PeerIdentity, PeerLocator, PeerStore};

pub(super) struct TransportPair {
    pub(super) id_a: Identity,
    pub(super) id_b: Identity,
    pub(super) directory_a: PeerDirectory,
    pub(super) directory_b: PeerDirectory,
    pub(super) transport_a: ConnectionManager,
    pub(super) transport_b: ConnectionManager,
    _dir_a: TempDir,
    _dir_b: TempDir,
}

pub(super) async fn make_transport_pair() -> TransportPair {
    make_transport_pair_with_options(128, 128, None).await
}

pub(super) async fn make_transport_pair_with_options(
    max_connections_a: usize,
    max_connections_b: usize,
    response_handler_b: Option<ResponseHandlerFn>,
) -> TransportPair {
    let dir_a = tempdir().expect("tempdir a");
    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    let id_a = Identity::load_or_generate(&paths_a).expect("identity a");
    let dir_b = tempdir().expect("tempdir b");
    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

    let agent_a = AgentId::parse(id_a.agent_id()).expect("agent a");
    let agent_b = AgentId::parse(id_b.agent_id()).expect("agent b");
    let identity_a = PeerIdentity::from_public_key(id_a.public_key_base64()).expect("peer a");
    let identity_b = PeerIdentity::from_public_key(id_b.public_key_base64()).expect("peer b");
    let directory_a = PeerDirectory::load(agent_a, PeerStore::new(paths_a.peers.clone()))
        .await
        .expect("directory a");
    let directory_b = PeerDirectory::load(agent_b, PeerStore::new(paths_b.peers.clone()))
        .await
        .expect("directory b");
    directory_a
        .enroll(identity_b.clone(), Vec::new())
        .await
        .expect("enroll b");
    directory_b
        .enroll(identity_a.clone(), Vec::new())
        .await
        .expect("enroll a");

    let transport_b = ConnectionManager::bind_cancellable(
        "127.0.0.1:0".parse().unwrap(),
        &id_b,
        CancellationToken::new(),
        max_connections_b,
        Duration::from_secs(15),
        Duration::from_secs(60),
        response_handler_b,
        Duration::from_secs(10),
        directory_b.pinning_snapshot(),
    )
    .await
    .expect("bind b");
    let transport_a = ConnectionManager::bind_cancellable(
        "127.0.0.1:0".parse().unwrap(),
        &id_a,
        CancellationToken::new(),
        max_connections_a,
        Duration::from_secs(15),
        Duration::from_secs(60),
        None,
        Duration::from_secs(10),
        directory_a.pinning_snapshot(),
    )
    .await
    .expect("bind a");
    directory_a
        .enroll(
            identity_b,
            vec![PeerLocator::Socket(transport_b.local_addr().unwrap())],
        )
        .await
        .expect("locate b");
    directory_b
        .enroll(
            identity_a,
            vec![PeerLocator::Socket(transport_a.local_addr().unwrap())],
        )
        .await
        .expect("locate a");

    TransportPair {
        id_a,
        id_b,
        directory_a,
        directory_b,
        transport_a,
        transport_b,
        _dir_a: dir_a,
        _dir_b: dir_b,
    }
}
