use std::path::{Path, PathBuf};
use std::time::Duration;

use axon::config::AxonPaths;
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;
use axon::message::AgentId;
use axon::peer_directory::{PeerDirectory, PeerIdentity, PeerLocator, PeerStore};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct RunningDaemon {
    paths: AxonPaths,
    identity: Identity,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    _root: TempDir,
}

impl RunningDaemon {
    async fn stop(self) {
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("shutdown timeout")
            .expect("daemon task")
            .expect("daemon result");
    }
}

async fn prepare_pair() -> (RunningDaemon, RunningDaemon) {
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();
    let paths_a = AxonPaths::from_root(PathBuf::from(root_a.path()));
    let paths_b = AxonPaths::from_root(PathBuf::from(root_b.path()));
    paths_a.ensure_root_exists().unwrap();
    paths_b.ensure_root_exists().unwrap();
    let identity_a = Identity::load_or_generate(&paths_a).unwrap();
    let identity_b = Identity::load_or_generate(&paths_b).unwrap();
    let port_a = free_port();
    let port_b = free_port();

    let directory_a = PeerDirectory::load(
        AgentId::parse(identity_a.agent_id()).unwrap(),
        PeerStore::new(paths_a.peers.clone()),
    )
    .await
    .unwrap();
    let directory_b = PeerDirectory::load(
        AgentId::parse(identity_b.agent_id()).unwrap(),
        PeerStore::new(paths_b.peers.clone()),
    )
    .await
    .unwrap();
    directory_a
        .enroll(
            PeerIdentity::from_public_key(identity_b.public_key_base64()).unwrap(),
            vec![PeerLocator::Socket(
                format!("127.0.0.1:{port_b}").parse().unwrap(),
            )],
        )
        .await
        .unwrap();
    directory_b
        .enroll(
            PeerIdentity::from_public_key(identity_a.public_key_base64()).unwrap(),
            vec![PeerLocator::Socket(
                format!("127.0.0.1:{port_a}").parse().unwrap(),
            )],
        )
        .await
        .unwrap();

    let daemon_a = spawn(root_a, paths_a, identity_a, port_a);
    let daemon_b = spawn(root_b, paths_b, identity_b, port_b);
    wait_for_socket(&daemon_a.paths.socket).await;
    wait_for_socket(&daemon_b.paths.socket).await;
    (daemon_a, daemon_b)
}

fn spawn(root: TempDir, paths: AxonPaths, identity: Identity, port: u16) -> RunningDaemon {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let root_path = root.path().to_path_buf();
    let task = tokio::spawn(async move {
        run_daemon(DaemonOptions {
            port: Some(port),
            disable_mdns: true,
            axon_root: Some(root_path),
            cancel: Some(task_cancel),
        })
        .await
    });
    RunningDaemon {
        paths,
        identity,
        cancel,
        task,
        _root: root,
    }
}

async fn wait_for_socket(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "socket did not appear"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
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
