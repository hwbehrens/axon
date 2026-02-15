use crate::*;

// =========================================================================
// §2 Peer table contention
// =========================================================================

/// 50 tasks do random peer table operations concurrently.
/// Must complete without panics or deadlocks.
#[tokio::test]
async fn peer_table_contention() {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let table = PeerTable::new();
        let ids = random_agent_ids(20);

        let mut handles = Vec::new();
        for task_id in 0..50u32 {
            let table = table.clone();
            let ids = ids.clone();
            handles.push(tokio::spawn(async move {
                for iter in 0..20u32 {
                    let idx = ((task_id as usize).wrapping_mul(7) + iter as usize) % ids.len();
                    let id = &ids[idx];
                    let op = (task_id.wrapping_add(iter)) % 7;

                    match op {
                        0 => {
                            table
                                .upsert_discovered(
                                    id.as_str().into(),
                                    "127.0.0.1:7100".parse().unwrap(),
                                    "cHVia2V5".to_string(),
                                )
                                .await;
                        }
                        1 => {
                            table.set_status(id, ConnectionStatus::Connecting).await;
                        }
                        2 => {
                            table.set_connected(id, Some(1.0)).await;
                        }
                        3 => {
                            table.set_disconnected(id).await;
                        }
                        4 => {
                            let _ = table.list().await;
                        }
                        5 => {
                            let _ = table.get(id).await;
                        }
                        6 => {
                            let _ = table.remove(id).await;
                        }
                        _ => unreachable!(),
                    }
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Consistency check: list and get must agree.
        let listed = table.list().await;
        for record in &listed {
            let got = table.get(&record.agent_id).await;
            assert!(
                got.is_some(),
                "listed peer {} must be found via get()",
                record.agent_id
            );
        }
    })
    .await;

    assert!(result.is_ok(), "peer_table_contention timed out (10s)");
}
