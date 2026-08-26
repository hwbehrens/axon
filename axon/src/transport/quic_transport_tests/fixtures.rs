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
        directory_b.clone(),
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
        directory_a.clone(),
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

#[allow(dead_code)] // id_b/id_c pin the fixture's three identities for future tests
pub(super) struct TransportTrio {
    pub(super) id_a: Identity,
    pub(super) id_b: Identity,
    pub(super) id_c: Identity,
    pub(super) directory_a: PeerDirectory,
    pub(super) transport_a: ConnectionManager,
    pub(super) agent_b: AgentId,
    pub(super) agent_c: AgentId,
    _dir_a: TempDir,
    _dir_b: TempDir,
    _dir_c: TempDir,
}

/// A↔B↔C fixture: A has B and C enrolled with live locators; B and C each
/// have A enrolled. Used for cross-peer interference regressions (revoking
/// one peer must not affect another's handshake or slot).
pub(super) async fn make_transport_trio() -> TransportTrio {
    let roots: Vec<TempDir> = (0..3).map(|_| tempdir().expect("tempdir")).collect();
    let paths: Vec<AxonPaths> = roots
        .iter()
        .map(|root| AxonPaths::from_root(root.path().to_path_buf()))
        .collect();
    let ids: Vec<Identity> = paths
        .iter()
        .map(|paths| Identity::load_or_generate(paths).expect("identity"))
        .collect();
    let agents: Vec<AgentId> = ids
        .iter()
        .map(|id| AgentId::parse(id.agent_id()).expect("agent"))
        .collect();
    let identities: Vec<PeerIdentity> = ids
        .iter()
        .map(|id| PeerIdentity::from_public_key(id.public_key_base64()).expect("peer identity"))
        .collect();

    let mut directories = Vec::new();
    for index in 0..3 {
        directories.push(
            PeerDirectory::load(
                agents[index].clone(),
                PeerStore::new(paths[index].peers.clone()),
            )
            .await
            .expect("directory"),
        );
    }

    let mut transports = Vec::new();
    for index in 0..3 {
        transports.push(
            ConnectionManager::bind_cancellable(
                "127.0.0.1:0".parse().unwrap(),
                &ids[index],
                CancellationToken::new(),
                128,
                Duration::from_secs(15),
                Duration::from_secs(60),
                None,
                Duration::from_secs(10),
                directories[index].clone(),
            )
            .await
            .expect("bind"),
        );
    }
    let local_addrs: Vec<std::net::SocketAddr> = transports
        .iter()
        .map(|transport| transport.local_addr().expect("local addr"))
        .collect();

    // A enrolls B and C with live locators.
    for peer in 1..3 {
        directories[0]
            .enroll(
                identities[peer].clone(),
                vec![PeerLocator::Socket(local_addrs[peer])],
            )
            .await
            .expect("enroll on A");
    }
    // B and C enroll A with live locators so inbound direction works too.
    for directory in directories.iter().skip(1) {
        directory
            .enroll(
                identities[0].clone(),
                vec![PeerLocator::Socket(local_addrs[0])],
            )
            .await
            .expect("enroll A");
    }

    let mut roots_iter = roots.into_iter();
    TransportTrio {
        id_a: ids[0].clone(),
        id_b: ids[1].clone(),
        id_c: ids[2].clone(),
        directory_a: directories[0].clone(),
        transport_a: transports[0].clone(),
        agent_b: agents[1].clone(),
        agent_c: agents[2].clone(),
        _dir_a: roots_iter.next().expect("root a"),
        _dir_b: roots_iter.next().expect("root b"),
        _dir_c: roots_iter.next().expect("root c"),
    }
}
