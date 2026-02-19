use super::*;

#[tokio::test]
async fn static_discovery_emits_all_peers() {
    let peers = vec![
        StaticPeerConfig {
            agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            addr: "127.0.0.1:7100".parse().expect("addr"),
            pubkey: "Zm9v".to_string(),
        },
        StaticPeerConfig {
            agent_id: "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            addr: "127.0.0.1:7101".parse().expect("addr"),
            pubkey: "YmFy".to_string(),
        },
    ];

    let (tx, mut rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();

    tokio::spawn(async move {
        let _ = run_static_discovery(peers, tx, cancel).await;
    });

    let first = rx.recv().await.expect("first event");
    let second = rx.recv().await.expect("second event");

    match first {
        PeerEvent::Discovered { agent_id, .. } => {
            assert_eq!(agent_id, "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        }
        _ => panic!("expected Discovered"),
    }
    match second {
        PeerEvent::Discovered { agent_id, .. } => {
            assert_eq!(agent_id, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        }
        _ => panic!("expected Discovered"),
    }
}

#[tokio::test]
async fn static_discovery_stays_alive() {
    let peers = vec![StaticPeerConfig {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: "127.0.0.1:7100".parse().expect("addr"),
        pubkey: "Zm9v".to_string(),
    }];

    let (tx, mut rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();

    let handle = tokio::spawn(async move { run_static_discovery(peers, tx, cancel).await });

    rx.recv().await.expect("should receive event");
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    assert!(!handle.is_finished());
    handle.abort();
}

#[test]
fn parse_resolved_ignores_self() {
    let props = [
        ("agent_id", "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("pubkey", "Zm9v"),
    ];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "axon-a",
        "axon-a.local.",
        "10.1.1.10",
        7100,
        &props[..],
    )
    .expect("service info");

    let parsed =
        parse_resolved_service("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
    assert!(parsed.is_none());
}

#[test]
fn parse_resolved_extracts_peer() {
    let props = [
        ("agent_id", "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ("pubkey", "YmFy"),
    ];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "axon-b",
        "axon-b.local.",
        "10.1.1.11",
        7101,
        &props[..],
    )
    .expect("service info");

    let parsed =
        parse_resolved_service("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
    let (event, _fullname, agent_id) = parsed.expect("expected discovered peer");

    assert_eq!(agent_id, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    match event {
        PeerEvent::Discovered {
            agent_id,
            addr,
            pubkey,
        } => {
            assert_eq!(agent_id, "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
            assert_eq!(addr.to_string(), "10.1.1.11:7101");
            assert_eq!(pubkey, "YmFy");
        }
        _ => panic!("expected Discovered"),
    }
}

#[test]
fn parse_resolved_skips_missing_agent_id() {
    let props: [(&str, &str); 0] = [];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "axon-x",
        "axon-x.local.",
        "10.1.1.12",
        7100,
        &props[..],
    )
    .expect("service info");

    let parsed =
        parse_resolved_service("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
    assert!(parsed.is_none());
}

#[test]
fn parse_resolved_skips_missing_pubkey() {
    let props = [("agent_id", "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "axon-b",
        "axon-b.local.",
        "10.1.1.13",
        7100,
        &props[..],
    )
    .expect("service info");

    let parsed =
        parse_resolved_service("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &info).expect("parse");
    assert!(parsed.is_none());
}
