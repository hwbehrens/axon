use std::path::{Path, PathBuf};
use std::time::Duration;

use axon::config::AxonPaths;
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;
use axon::ipc::MAX_IPC_LINE_LENGTH;
use axon::message::AgentId;
use axon::peer_directory::{PeerDirectory, PeerIdentity, PeerLocator, PeerStore};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

mod capabilities;
mod drain;
mod enrollment;
mod lease;
mod ports;
mod saturation;
mod traffic;

use ports::{PORT_BIND_ATTEMPTS, free_port, is_port_conflict, spawn_ready, wait_for_ready};

struct RunningDaemon {
    paths: AxonPaths,
    identity: Identity,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    root: TempDir,
}

impl RunningDaemon {
    async fn stop(mut self) {
        self.cancel.cancel();
        let task = self.task.take().expect("daemon task already joined");
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("shutdown timeout")
            .expect("daemon task")
            .expect("daemon result");
    }

    /// Cancel and join without asserting the daemon's own result; used by
    /// teardown paths that race in-flight sends.
    async fn cancel_and_join(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(30), task).await;
        }
    }

    /// Reclaim the identity, paths, and root after a failed startup wait so
    /// a retry can reuse the caller's seeded state with a fresh port.
    async fn into_parts(mut self) -> (TempDir, AxonPaths, Identity) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
        (self.root, self.paths, self.identity)
    }
}

async fn prepare_pair() -> (RunningDaemon, RunningDaemon) {
    prepare_pair_with(free_port, free_port, None).await
}

/// Start a mutually-enrolled pair, retrying the whole pair with fresh ports
/// when a daemon loses the probe-to-bind race. The retry re-enrolls: the
/// persisted locators must name the ports the daemons actually bound, so
/// only the pairing as a whole can be recreated. `budget_a` injects daemon
/// A's send budget for capacity tests.
async fn prepare_pair_with(
    mut port_a: impl FnMut() -> u16,
    mut port_b: impl FnMut() -> u16,
    budget_a: Option<usize>,
) -> (RunningDaemon, RunningDaemon) {
    let mut attempt = 1;
    loop {
        let pending = prepare_identities_with(&mut port_a, &mut port_b);
        enroll_pair(&pending).await;
        match start_pair(pending, budget_a).await {
            Ok(pair) => return pair,
            Err(error) if attempt < PORT_BIND_ATTEMPTS && is_port_conflict(&error) => {
                eprintln!(
                    "[diag] pair startup hit a port conflict (attempt {attempt}/{PORT_BIND_ATTEMPTS}): \
                     {error:#}; retrying with fresh ports"
                );
                attempt += 1;
            }
            Err(error) => {
                panic!("daemon pair failed to start after {attempt} attempt(s): {error:#}")
            }
        }
    }
}

/// Enroll both identities with each other's locator. Enrollment must land in
/// peers.json before the daemons load it.
async fn enroll_pair(pending: &PendingPair) {
    let directory_a = PeerDirectory::load(
        AgentId::parse(pending.identity_a.agent_id()).unwrap(),
        PeerStore::new(pending.paths_a.peers.clone()),
    )
    .await
    .unwrap();
    let directory_b = PeerDirectory::load(
        AgentId::parse(pending.identity_b.agent_id()).unwrap(),
        PeerStore::new(pending.paths_b.peers.clone()),
    )
    .await
    .unwrap();
    directory_a
        .enroll(
            PeerIdentity::from_public_key(pending.identity_b.public_key_base64()).unwrap(),
            vec![PeerLocator::Socket(
                format!("127.0.0.1:{}", pending.port_b).parse().unwrap(),
            )],
        )
        .await
        .unwrap();
    directory_b
        .enroll(
            PeerIdentity::from_public_key(pending.identity_a.public_key_base64()).unwrap(),
            vec![PeerLocator::Socket(
                format!("127.0.0.1:{}", pending.port_a).parse().unwrap(),
            )],
        )
        .await
        .unwrap();
    drop(directory_a);
    drop(directory_b);
}

/// Identities, paths, and ports prepared but not yet started, so callers
/// can seed peer state before the daemons load it.
struct PendingPair {
    root_a: TempDir,
    root_b: TempDir,
    paths_a: AxonPaths,
    paths_b: AxonPaths,
    identity_a: Identity,
    identity_b: Identity,
    port_a: u16,
    port_b: u16,
}

fn prepare_identities() -> PendingPair {
    prepare_identities_with(free_port, free_port)
}

fn prepare_identities_with(
    mut port_a: impl FnMut() -> u16,
    mut port_b: impl FnMut() -> u16,
) -> PendingPair {
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();
    let paths_a = AxonPaths::from_root(PathBuf::from(root_a.path()));
    let paths_b = AxonPaths::from_root(PathBuf::from(root_b.path()));
    paths_a.ensure_root_exists().unwrap();
    paths_b.ensure_root_exists().unwrap();
    let identity_a = Identity::load_or_generate(&paths_a).unwrap();
    let identity_b = Identity::load_or_generate(&paths_b).unwrap();
    PendingPair {
        root_a,
        root_b,
        paths_a,
        paths_b,
        identity_a,
        identity_b,
        port_a: port_a(),
        port_b: port_b(),
    }
}

async fn start_pair(
    pending: PendingPair,
    budget_a: Option<usize>,
) -> anyhow::Result<(RunningDaemon, RunningDaemon)> {
    let PendingPair {
        root_a,
        root_b,
        paths_a,
        paths_b,
        identity_a,
        identity_b,
        port_a,
        port_b,
    } = pending;
    let mut daemon_a = spawn_with_budget(root_a, paths_a, identity_a, port_a, budget_a);
    let mut daemon_b = spawn(root_b, paths_b, identity_b, port_b);
    let ready_a = wait_for_ready(&mut daemon_a).await;
    let ready_b = wait_for_ready(&mut daemon_b).await;
    match (ready_a, ready_b) {
        (Ok(()), Ok(())) => Ok((daemon_a, daemon_b)),
        (Err(error), _) | (_, Err(error)) => {
            // A ready sibling must not keep running (or keep holding its
            // port) while the caller retries the pair.
            drop(daemon_a.into_parts().await);
            drop(daemon_b.into_parts().await);
            Err(error)
        }
    }
}

/// Spawn two daemons with fresh identities but NO enrolled peers.
async fn spawn_pair() -> (RunningDaemon, RunningDaemon, Identity, Identity, u16, u16) {
    let mut attempt = 1;
    loop {
        let pending = prepare_identities();
        let identity_a = pending.identity_a.clone();
        let identity_b = pending.identity_b.clone();
        let port_a = pending.port_a;
        let port_b = pending.port_b;
        match start_pair(pending, None).await {
            Ok((daemon_a, daemon_b)) => {
                return (daemon_a, daemon_b, identity_a, identity_b, port_a, port_b);
            }
            Err(error) if attempt < PORT_BIND_ATTEMPTS && is_port_conflict(&error) => {
                eprintln!(
                    "[diag] pair startup hit a port conflict (attempt {attempt}/{PORT_BIND_ATTEMPTS}): \
                     {error:#}; retrying with fresh ports"
                );
                attempt += 1;
            }
            Err(error) => {
                panic!("daemon pair failed to start after {attempt} attempt(s): {error:#}")
            }
        }
    }
}

fn spawn(root: TempDir, paths: AxonPaths, identity: Identity, port: u16) -> RunningDaemon {
    spawn_with_budget(root, paths, identity, port, None)
}

fn spawn_with_budget(
    root: TempDir,
    paths: AxonPaths,
    identity: Identity,
    port: u16,
    max_inflight_sends: Option<usize>,
) -> RunningDaemon {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let root_path = root.path().to_path_buf();
    let task = tokio::spawn(async move {
        run_daemon(DaemonOptions {
            port: Some(port),
            disable_mdns: true,
            axon_root: Some(root_path),
            cancel: Some(task_cancel),
            max_inflight_sends,
        })
        .await
    });
    RunningDaemon {
        paths,
        identity,
        cancel,
        task: Some(task),
        root,
    }
}

async fn ipc_command(path: &Path, command: Value) -> Value {
    let mut stream = UnixStream::connect(path).await.unwrap();
    stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn read_json(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Value {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("IPC read timeout")
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

/// Poll `peers` until `agent_id` reports `connected`, failing after the
/// timeout so missing connectivity cannot hang the suite.
pub(crate) async fn wait_for_peer_connected(path: &Path, agent_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "peer {agent_id} never reached connected state"
        );
        let reply = ipc_command(path, json!({ "cmd": "peers" })).await;
        if reply["peers"].as_array().is_some_and(|peers| {
            peers.iter().any(|peer| {
                peer["agent_id"].as_str() == Some(agent_id)
                    && peer["status"].as_str() == Some("connected")
            })
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn request_handler_replies_on_original_quic_stream_and_revocation_is_immediate() {
    let (daemon_a, daemon_b) = prepare_pair().await;
    let peer_a = daemon_a.identity.agent_id().to_string();
    let peer_b = daemon_b.identity.agent_id().to_string();

    let handler = UnixStream::connect(&daemon_b.paths.socket).await.unwrap();
    let (handler_read, mut handler_write) = handler.into_split();
    let mut handler_reader = BufReader::new(handler_read);
    handler_write
        .write_all(b"{\"cmd\":\"serve\"}\n")
        .await
        .unwrap();
    assert_eq!(read_json(&mut handler_reader).await["serving"], true);

    let socket_a = daemon_a.paths.socket.clone();
    let peer_b_for_request = peer_b.clone();
    let outbound = tokio::spawn(async move {
        ipc_command(
            &socket_a,
            json!({
                "cmd": "send",
                "to": peer_b_for_request,
                "kind": "request",
                "payload": {"question": "ready?"},
                "timeout_secs": 5
            }),
        )
        .await
    });
    let request = read_json(&mut handler_reader).await;
    assert_eq!(request["event"], "request");
    assert_eq!(request["from"], peer_a);
    handler_write
        .write_all(
            format!(
                "{}\n",
                json!({
                    "cmd": "reply",
                    "request_id": request["request_id"],
                    "kind": "response",
                    "payload": {"answer": "yes"}
                })
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert_eq!(read_json(&mut handler_reader).await["ok"], true);
    let response = outbound.await.unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["response"]["kind"], "response");
    assert_eq!(response["response"]["payload"]["answer"], "yes");

    let revoked = ipc_command(
        &daemon_a.paths.socket,
        json!({"cmd": "remove_peer", "agent_id": peer_b}),
    )
    .await;
    assert_eq!(revoked["ok"], true);
    let rejected = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": daemon_b.identity.agent_id(),
            "kind": "message",
            "payload": {"after": "revoke"}
        }),
    )
    .await;
    assert_eq!(rejected["error"], "peer_not_found");

    daemon_a.stop().await;
    daemon_b.stop().await;
}
