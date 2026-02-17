//! Integration tests — cross-module interactions.
//!
//! These tests exercise multiple subsystems together without starting
//! a full daemon process.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axon::config::{
    AxonPaths, Config, KnownPeer, StaticPeerConfig, load_known_peers, save_known_peers,
};
use axon::discovery::{PeerEvent, run_static_discovery};
use axon::identity::Identity;
use axon::ipc::{DaemonReply, IpcCommand, IpcServer, IpcServerConfig};
use axon::message::{AgentId, Envelope, MessageKind, decode, encode};
use axon::peer_table::{ConnectionStatus, PeerSource, PeerTable};
use axon::transport::QuicTransport;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod integration {
    pub(crate) mod discovery;
    pub(crate) mod identity;
    pub(crate) mod ipc;
    pub(crate) mod transport;
    pub(crate) mod violations;
}

// =========================================================================
// Helpers
// =========================================================================

pub(crate) fn make_identity() -> (Identity, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let id = Identity::load_or_generate(&paths).unwrap();
    (id, dir)
}

pub(crate) fn make_peer_record(
    id: &Identity,
    addr: std::net::SocketAddr,
) -> axon::peer_table::PeerRecord {
    axon::peer_table::PeerRecord {
        agent_id: id.agent_id().into(),
        addr,
        pubkey: id.public_key_base64().to_string(),
        source: PeerSource::Static,
        status: ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: Instant::now(),
    }
}

/// Create a pair of transports with mutual pubkey registration via PeerTable.
pub(crate) async fn make_transport_pair(
    id_a: &Identity,
    id_b: &Identity,
) -> (QuicTransport, QuicTransport, PeerTable, PeerTable) {
    let table_a = PeerTable::new();
    let table_b = PeerTable::new();

    // Register each peer's pubkey in the other's table
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

    let transport_b = QuicTransport::bind(
        "127.0.0.1:0".parse().unwrap(),
        id_b,
        128,
        table_b.pubkey_map(),
    )
    .await
    .unwrap();

    let transport_a = QuicTransport::bind(
        "127.0.0.1:0".parse().unwrap(),
        id_a,
        128,
        table_a.pubkey_map(),
    )
    .await
    .unwrap();

    (transport_a, transport_b, table_a, table_b)
}
