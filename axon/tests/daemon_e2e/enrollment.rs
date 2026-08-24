//! Full-stack peer lifecycle behavior: runtime enrollment through IPC,
//! unreachable enrolled peers, and daemon resilience during reconnect
//! churn.
//!
//! Restores connection-lifecycle e2e coverage removed by the
//! peer-directory redesign, adapted to intentional enrollment (DEC-012)
//! and generation-checked reconnect (DEC-014).

use axon::peer_directory::{
    MAX_ENROLLED_PEERS, PeerDirectory, PeerIdentity, PeerLocator, PeerStore,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::*;

/// `add_peer` with a peer token enrolls durably and produces a working
/// connection without restarting either daemon (Q-010: IPC mutations are
/// the only live authority path).
#[tokio::test]
async fn add_peer_via_ipc_connects_without_restart() {
    let (daemon_a, daemon_b, identity_a, identity_b, port_a, port_b) = spawn_pair().await;

    // Before enrollment, sends are rejected as unknown peers.
    let rejected = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": identity_b.agent_id(),
            "kind": "message",
            "payload": {"before": "enrollment"}
        }),
    )
    .await;
    assert_eq!(rejected["error"], "peer_not_found");

    let token_for_b = axon::peer_token::encode(
        identity_b.public_key_base64(),
        &format!("127.0.0.1:{port_b}"),
    )
    .expect("valid peer token");
    let token_for_a = axon::peer_token::encode(
        identity_a.public_key_base64(),
        &format!("127.0.0.1:{port_a}"),
    )
    .expect("valid peer token");
    let enrolled_a = ipc_command(
        &daemon_a.paths.socket,
        json!({ "cmd": "add_peer", "token": token_for_b }),
    )
    .await;
    assert_eq!(enrolled_a["ok"], json!(true), "{enrolled_a}");
    let enrolled_b = ipc_command(
        &daemon_b.paths.socket,
        json!({ "cmd": "add_peer", "token": token_for_a }),
    )
    .await;
    assert_eq!(enrolled_b["ok"], json!(true), "{enrolled_b}");

    wait_for_peer_connected(&daemon_a.paths.socket, identity_b.agent_id()).await;

    let ack = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": identity_b.agent_id(),
            "kind": "message",
            "payload": {"after": "enrollment"}
        }),
    )
    .await;
    assert_eq!(ack["ok"], json!(true), "send after enrollment must work");

    // Enrollment is durable: the peer store on disk records the peer.
    let stored = std::fs::read_to_string(&daemon_a.paths.peers).unwrap();
    assert!(
        stored.contains(identity_b.agent_id()),
        "peers.json must record the enrolled Agent ID"
    );

    daemon_a.stop().await;
    daemon_b.stop().await;
}

/// An enrolled but unreachable peer fails sends with an instructive error
/// instead of hanging or silently dropping.
#[tokio::test]
async fn send_to_unreachable_enrolled_peer_returns_error() {
    let root = tempfile::tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    paths.ensure_root_exists().unwrap();
    let identity = Identity::load_or_generate(&paths).unwrap();

    // Enroll a peer whose port has no listener.
    let dead_port = free_port();
    let remote_key = STANDARD.encode([7u8; 32]);
    let remote_id = axon::peer_token::derive_agent_id_from_pubkey_base64(&remote_key).unwrap();
    let directory = PeerDirectory::load(
        AgentId::parse(identity.agent_id()).unwrap(),
        PeerStore::new(paths.peers.clone()),
    )
    .await
    .unwrap();
    directory
        .enroll(
            PeerIdentity::from_public_key(&remote_key).unwrap(),
            vec![PeerLocator::Socket(
                format!("127.0.0.1:{dead_port}").parse().unwrap(),
            )],
        )
        .await
        .unwrap();
    drop(directory);

    let daemon = spawn(root, paths, identity.clone(), free_port());
    wait_for_socket(&daemon.paths.socket).await;

    let reply = ipc_command(
        &daemon.paths.socket,
        json!({
            "cmd": "send",
            "to": remote_id.as_str(),
            "kind": "message",
            "payload": {}
        }),
    )
    .await;
    assert_eq!(
        reply["error"], "peer_unreachable",
        "unreachable enrolled peer must produce a typed error: {reply}"
    );

    daemon.stop().await;
}

/// While the reconnect loop churns against an unreachable enrolled peer,
/// IPC stays responsive, and shutdown remains bounded even with a client
/// connected.
#[tokio::test]
async fn ipc_stays_responsive_during_reconnect_churn_and_shutdown_is_bounded() {
    let root = tempfile::tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    paths.ensure_root_exists().unwrap();
    let identity = Identity::load_or_generate(&paths).unwrap();

    let dead_port = free_port();
    let remote_key = STANDARD.encode([9u8; 32]);
    let directory = PeerDirectory::load(
        AgentId::parse(identity.agent_id()).unwrap(),
        PeerStore::new(paths.peers.clone()),
    )
    .await
    .unwrap();
    directory
        .enroll(
            PeerIdentity::from_public_key(&remote_key).unwrap(),
            vec![PeerLocator::Socket(
                format!("127.0.0.1:{dead_port}").parse().unwrap(),
            )],
        )
        .await
        .unwrap();
    drop(directory);

    let daemon = spawn(root, paths, identity.clone(), free_port());
    wait_for_socket(&daemon.paths.socket).await;

    // Hold an IPC client open across the churn and the shutdown.
    let client = UnixStream::connect(&daemon.paths.socket).await.unwrap();
    let (mut client_read, mut client_write) = client.into_split();
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    client_write
        .write_all(b"{\"cmd\":\"status\"}\n")
        .await
        .unwrap();
    let mut reader = BufReader::new(&mut client_read);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"ok\":true"));

    for _ in 0..5 {
        let reply = ipc_command(&daemon.paths.socket, json!({ "cmd": "status" })).await;
        assert_eq!(
            reply["ok"],
            json!(true),
            "IPC must stay responsive during reconnect churn"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let shutdown = tokio::time::timeout(Duration::from_secs(6), daemon.stop()).await;
    assert!(
        shutdown.is_ok(),
        "shutdown must stay bounded while reconnect churn and IPC clients are active"
    );
}

/// Enrollment enforces the durable bound: exceeding MAX_ENROLLED_PEERS is
/// rejected instead of silently growing the trust set.
#[tokio::test]
async fn enrollment_rejects_peers_beyond_the_configured_bound() {
    let root = tempfile::tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    paths.ensure_root_exists().unwrap();
    let identity = Identity::load_or_generate(&paths).unwrap();

    let directory = PeerDirectory::load(
        AgentId::parse(identity.agent_id()).unwrap(),
        PeerStore::new(paths.peers.clone()),
    )
    .await
    .unwrap();
    for seed in 0..MAX_ENROLLED_PEERS {
        // Distinct 32-byte keys via a unique big-endian index prefix; the
        // Agent ID derives from the key, so every enrollment is a new
        // identity.
        let mut key_bytes = [0u8; 32];
        key_bytes[..2].copy_from_slice(&(seed as u16).to_be_bytes());
        let peer = PeerIdentity::from_public_key(&STANDARD.encode(key_bytes)).unwrap();
        directory
            .enroll(peer.clone(), Vec::new())
            .await
            .unwrap_or_else(|err| panic!("enroll {seed} failed: {err}"));
    }

    let overflow = PeerIdentity::from_public_key(&STANDARD.encode([255u8; 32])).unwrap();
    let result = directory.enroll(overflow, Vec::new()).await;
    assert!(
        result.is_err(),
        "enrollment beyond MAX_ENROLLED_PEERS must be rejected"
    );
}
