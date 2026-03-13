use super::super::{QuicTransport, ResponseHandlerFn};
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::peer_table::{ConnectionStatus, PeerRecord, PeerSource, PeerTable};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::{TempDir, tempdir};
use tokio::sync::RwLock;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

pub(super) struct TransportPair {
    pub(super) id_a: Identity,
    pub(super) id_b: Identity,
    pub(super) transport_a: QuicTransport,
    pub(super) transport_b: QuicTransport,
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

    let table_a = PeerTable::new();
    let table_b = PeerTable::new();

    table_a
        .upsert_discovered(
            id_b.agent_id().into(),
            "127.0.0.1:1".parse().unwrap(),
            id_b.public_key_base64().to_string(),
        )
        .await;
    table_b
        .upsert_discovered(
            id_a.agent_id().into(),
            "127.0.0.1:1".parse().unwrap(),
            id_a.public_key_base64().to_string(),
        )
        .await;

    let transport_b = QuicTransport::bind_cancellable(
        "127.0.0.1:0".parse().unwrap(),
        &id_b,
        CancellationToken::new(),
        max_connections_b,
        Duration::from_secs(15),
        Duration::from_secs(60),
        response_handler_b,
        Duration::from_secs(10),
        table_b.pubkey_map(),
    )
    .await
    .expect("bind b");
    let transport_a = QuicTransport::bind_cancellable(
        "127.0.0.1:0".parse().unwrap(),
        &id_a,
        CancellationToken::new(),
        max_connections_a,
        Duration::from_secs(15),
        Duration::from_secs(60),
        None,
        Duration::from_secs(10),
        table_a.pubkey_map(),
    )
    .await
    .expect("bind a");

    TransportPair {
        id_a,
        id_b,
        transport_a,
        transport_b,
        _dir_a: dir_a,
        _dir_b: dir_b,
    }
}

pub(super) fn peer_record(identity: &Identity, addr: SocketAddr) -> PeerRecord {
    PeerRecord {
        agent_id: identity.agent_id().into(),
        addr,
        pubkey: identity.public_key_base64().to_string(),
        source: PeerSource::Static,
        status: ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: std::time::Instant::now(),
    }
}

pub(super) async fn wait_for_registered_connection(
    connections: &Arc<RwLock<HashMap<String, quinn::Connection>>>,
    peer_id: &str,
    expected_stable_id: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if connections
            .read()
            .await
            .get(peer_id)
            .is_some_and(|c| c.stable_id() == expected_stable_id)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for connection registration"
        );
        sleep(Duration::from_millis(20)).await;
    }
}
