//! Integration tests — cross-module interactions.
//!
//! These tests exercise multiple subsystems together without starting
//! a full daemon process.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axon::config::{
    AxonPaths, Config, KnownPeer, StaticPeerConfig, load_known_peers, save_known_peers,
};
use axon::discovery::{Discovery, PeerEvent, StaticDiscovery};
use axon::identity::Identity;
use axon::ipc::{DaemonReply, IpcCommand, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MessageKind, decode, encode};
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
