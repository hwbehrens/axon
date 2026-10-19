//! Temporary probe (not committed): dedicated-runtime daemon scheduling.
use super::*;

struct ThreadedDaemon {
    paths: AxonPaths,
    cancel: CancellationToken,
    join: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    _root: TempDir,
}

impl ThreadedDaemon {
    async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(join) = self.join.take() {
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::task::spawn_blocking(move || join.join()),
            )
            .await;
        }
    }
}

fn spawn_threaded(
    root: TempDir,
    paths: AxonPaths,
    _identity: Identity,
    port: u16,
) -> ThreadedDaemon {
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let root_path = root.path().to_path_buf();
    let join = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(run_daemon(DaemonOptions {
            port: Some(port),
            disable_mdns: true,
            axon_root: Some(root_path),
            cancel: Some(c),
            max_inflight_sends: None,
        }))
    });
    ThreadedDaemon {
        paths,
        cancel,
        join: Some(join),
        _root: root,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_threaded() {
    let pending = prepare_identities();
    let identity_b = pending.identity_b.clone();
    let paths_b = pending.paths_b.clone();
    let port_b = pending.port_b;
    // Enroll A on B only (the probe only needs B's IPC).
    let directory_b = PeerDirectory::load(
        AgentId::parse(identity_b.agent_id()).unwrap(),
        PeerStore::new(paths_b.peers.clone()),
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
    drop(directory_b);
    drop(pending.identity_a.clone());
    let daemon = spawn_threaded(pending.root_b, paths_b.clone(), identity_b, port_b);
    let _keep_a_identity = pending.identity_a;
    let _keep_root_a = pending.root_a;
    wait_for_socket(&daemon.paths.socket).await;

    let socket = daemon.paths.socket.clone();
    let mut leaked = None;
    for trial in 1..=30 {
        let mut b = UnixStream::connect(&socket).await.unwrap();
        b.write_all(b"{\"cmd\":\"serve\"}\n").await.unwrap();
        drop(b);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let reply = ipc_command(&socket, json!({"cmd": "serve"})).await;
        if reply["ok"] == json!(true) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        // Re-check after a settle: only persistent busy means a leak.
        tokio::time::sleep(Duration::from_millis(800)).await;
        let again = ipc_command(&socket, json!({"cmd": "serve"})).await;
        if again["ok"] == json!(true) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        leaked = Some((trial, reply, again));
        break;
    }
    eprintln!("PROBE leaked={leaked:?}");
    assert!(leaked.is_none());
    daemon.stop().await;
}
