//! Unit tests for `ConnectionRegistry` admission selection.
//!
//! Uses real QUIC handshakes between two enrolled identities but drives the
//! endpoints manually (no accept loop), so duplicate connections stay alive
//! and the cross-dial replacement window can be exercised deterministically
//! — including the aged-incumbent case via an explicit zero window.

use std::net::SocketAddr;
use std::path::PathBuf;

use tempfile::{TempDir, tempdir};

use super::*;
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::peer_directory::{PeerDirectory, PeerIdentity, PeerStore};
use crate::transport::tls::{build_endpoint, with_handshake_remote_addr};

fn gate_ok() -> std::future::Ready<bool> {
    std::future::ready(true)
}

struct TestNode {
    identity: Identity,
    directory: PeerDirectory,
    /// Endpoints backing live test connections; quinn connections die when
    /// their endpoint is dropped or closed, so these must outlive them.
    _endpoints: std::sync::Mutex<Vec<quinn::Endpoint>>,
    _dir: TempDir,
}

impl TestNode {
    fn keep_alive(&self, mut endpoints: Vec<quinn::Endpoint>) {
        self._endpoints
            .lock()
            .expect("endpoint lock")
            .append(&mut endpoints);
    }
}

async fn test_node() -> TestNode {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let directory = PeerDirectory::load(
        AgentId::parse(identity.agent_id()).expect("agent id"),
        PeerStore::new(paths.peers),
    )
    .await
    .expect("directory");
    TestNode {
        identity,
        directory,
        _endpoints: std::sync::Mutex::new(Vec::new()),
        _dir: dir,
    }
}

fn new_endpoint(node: &TestNode) -> quinn::Endpoint {
    let (endpoint, _, _) = build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &node.identity.make_quic_certificate().expect("cert"),
        node.directory.pinning_snapshot(),
        Duration::from_secs(5),
        Duration::from_secs(30),
    )
    .expect("endpoint");
    endpoint
}

/// Owner is the lexicographically lower Agent ID, so the preferred direction
/// is always Outbound and an inbound incumbent is non-preferred.
/// The pair is cross-enrolled so TLS pinning accepts their handshakes.
async fn owner_and_peer() -> (TestNode, TestNode) {
    let a = test_node().await;
    let b = test_node().await;
    let (owner, peer) = if a.identity.agent_id() < b.identity.agent_id() {
        (a, b)
    } else {
        (b, a)
    };
    let owner_identity =
        PeerIdentity::from_public_key(owner.identity.public_key_base64()).expect("owner identity");
    let peer_identity =
        PeerIdentity::from_public_key(peer.identity.public_key_base64()).expect("peer identity");
    peer.directory
        .enroll(owner_identity, Vec::new())
        .await
        .expect("peer enrolls owner");
    owner
        .directory
        .enroll(peer_identity, Vec::new())
        .await
        .expect("owner enrolls peer");
    (owner, peer)
}

/// One completed handshake into `owner`: returns `(outbound at dialer,
/// inbound at owner)` as two distinct live connections.
async fn handshake_into(
    owner: &TestNode,
    dialer: &TestNode,
) -> (quinn::Connection, quinn::Connection) {
    let ep_owner = new_endpoint(owner);
    let ep_dialer = new_endpoint(dialer);
    let connecting = ep_dialer
        .connect(
            ep_owner.local_addr().expect("owner local addr"),
            owner.identity.agent_id(),
        )
        .expect("begin connect");
    let remote_addr = connecting.remote_address();
    let (outbound, inbound) =
        tokio::join!(with_handshake_remote_addr(remote_addr, connecting), async {
            let incoming = ep_owner.accept().await.expect("incoming");
            let addr: SocketAddr = incoming.remote_address();
            with_handshake_remote_addr(addr, incoming.into_future()).await
        });
    // Endpoints stay alive for as long as the returned connections are used.
    owner.keep_alive(vec![ep_owner]);
    dialer.keep_alive(vec![ep_dialer]);
    (
        outbound.expect("outbound handshake"),
        inbound.expect("inbound handshake"),
    )
}

#[tokio::test]
async fn empty_slot_admits_nonpreferred_direction() {
    let (owner, peer_node) = owner_and_peer().await;
    let peer = AgentId::parse(peer_node.identity.agent_id()).unwrap();
    let registry = ConnectionRegistry::new(AgentId::parse(owner.identity.agent_id()).unwrap());
    let (_out, inbound) = handshake_into(&owner, &peer_node).await;

    let admission = registry
        .admit_gated(peer.clone(), inbound.clone(), Direction::Inbound, gate_ok)
        .await;
    assert!(matches!(admission, Admission::Accepted { .. }));
    assert!(registry.current(&peer).await.is_some());
}

#[tokio::test]
async fn fresh_cross_dial_candidate_with_preferred_direction_replaces_incumbent() {
    // Simultaneous cross-dials converge: within the window, the preferred-
    // direction candidate replaces the non-preferred healthy incumbent
    // (Q-006/DEC-014 tie-break).
    let (owner, peer_node) = owner_and_peer().await;
    let peer = AgentId::parse(peer_node.identity.agent_id()).unwrap();
    let registry = ConnectionRegistry::new(AgentId::parse(owner.identity.agent_id()).unwrap());

    let (_out1, incumbent_conn) = handshake_into(&owner, &peer_node).await;
    let first = registry
        .admit_gated(
            peer.clone(),
            incumbent_conn.clone(),
            Direction::Inbound,
            gate_ok,
        )
        .await;
    assert!(matches!(first, Admission::Accepted { .. }));

    let (_out2, racer_conn) = handshake_into(&owner, &peer_node).await;
    let second = registry
        .admit_gated(
            peer.clone(),
            racer_conn.clone(),
            Direction::Outbound,
            gate_ok,
        )
        .await;
    assert!(
        matches!(second, Admission::Accepted { .. }),
        "preferred-direction candidate inside the window must win"
    );

    let current = registry.current(&peer).await.expect("authoritative slot");
    assert_eq!(current.stable_id(), racer_conn.stable_id());

    // The displaced loser must be closed so both sides converge on one
    // physical connection.
    let deadline = Instant::now() + Duration::from_secs(2);
    while incumbent_conn.close_reason().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        incumbent_conn.close_reason().is_some(),
        "displaced cross-dial loser must be closed"
    );
}

#[tokio::test]
async fn aged_healthy_incumbent_rejects_late_preferred_direction_candidate() {
    // SPEC.md §Connection Lifecycle 4: a healthy incumbent wins within its
    // generation. Once the cross-dial race window has closed, a preferred-
    // direction candidate is just a duplicate and must NOT evict a proven
    // connection solely because of its direction.
    let (owner, peer_node) = owner_and_peer().await;
    let peer = AgentId::parse(peer_node.identity.agent_id()).unwrap();
    let registry = ConnectionRegistry::new(AgentId::parse(owner.identity.agent_id()).unwrap());

    let (_out1, incumbent_conn) = handshake_into(&owner, &peer_node).await;
    assert!(matches!(
        registry
            .admit_gated(
                peer.clone(),
                incumbent_conn.clone(),
                Direction::Inbound,
                gate_ok
            )
            .await,
        Admission::Accepted { .. }
    ));

    // Aged incumbent: the race window has already exhausted when this
    // candidate arrives.
    let (_out2, late_candidate) = handshake_into(&owner, &peer_node).await;
    let admission = registry
        .admit_gated_with_window(
            peer.clone(),
            late_candidate,
            Direction::Outbound,
            gate_ok,
            Duration::ZERO,
        )
        .await;
    match admission {
        Admission::Existing(existing) => {
            assert_eq!(existing.stable_id(), incumbent_conn.stable_id());
        }
        _ => {
            panic!("late preferred-direction candidate must lose to the healthy incumbent")
        }
    }

    // The authoritative slot is untouched.
    let current = registry.current(&peer).await.expect("incumbent survives");
    assert_eq!(current.stable_id(), incumbent_conn.stable_id());
}
