use super::super::*;

use proptest::prelude::*;

#[derive(Debug, Clone)]
enum PeerOp {
    Insert(String),
    Remove(String),
    SetStatus(String, ConnectionStatus),
    SetConnected(String),
    SetDisconnected(String),
    List,
}

fn arb_peer_op() -> impl Strategy<Value = PeerOp> {
    let id_strategy = "[0-9a-f]{32}";
    prop_oneof![
        id_strategy.prop_map(PeerOp::Insert),
        id_strategy.prop_map(PeerOp::Remove),
        (
            id_strategy,
            prop::sample::select(vec![
                ConnectionStatus::Discovered,
                ConnectionStatus::Connecting,
                ConnectionStatus::Connected,
                ConnectionStatus::Disconnected,
            ])
        )
            .prop_map(|(id, s)| PeerOp::SetStatus(id, s)),
        id_strategy.prop_map(PeerOp::SetConnected),
        id_strategy.prop_map(PeerOp::SetDisconnected),
        Just(PeerOp::List),
    ]
}

proptest! {
    #[test]
    fn concurrent_insert_remove_never_panics(
        ops in proptest::collection::vec(arb_peer_op(), 1..50),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let table = PeerTable::new();
            let mut handles = Vec::new();
            for op in ops {
                let t = table.clone();
                handles.push(tokio::spawn(async move {
                    match op {
                        PeerOp::Insert(id) => {
                            t.upsert_discovered(
                                id.into(),
                                "127.0.0.1:7100".parse().unwrap(),
                                "Zm9v".to_string(),
                            ).await;
                        }
                        PeerOp::Remove(id) => { t.remove(&id).await; }
                        PeerOp::SetStatus(id, s) => { t.set_status(&id, s).await; }
                        PeerOp::SetConnected(id) => { t.set_connected(&id, Some(1.0)).await; }
                        PeerOp::SetDisconnected(id) => { t.set_disconnected(&id).await; }
                        PeerOp::List => { let _ = t.list().await; }
                    }
                }));
            }
            for h in handles {
                h.await.unwrap();
            }
            let listed = table.list().await;
            assert!(listed.len() <= 50);
        });
    }
}
