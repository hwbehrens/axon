use std::net::SocketAddr;
use std::time::Duration;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use axon::peer_table::PeerTable;

fn make_agent_id(i: usize) -> String {
    format!("{i:032x}")
}

fn make_pubkey(i: usize) -> String {
    format!("pubkey_{i:024}")
}

fn bench_upsert_discovered(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("peer_table_upsert");

    group.bench_function("single", |b| {
        b.iter(|| {
            rt.block_on(async {
                let table = PeerTable::new();
                let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
                table
                    .upsert_discovered(
                        black_box(make_agent_id(0)),
                        black_box(addr),
                        black_box(make_pubkey(0)),
                    )
                    .await;
            });
        })
    });

    group.bench_function("100_peers", |b| {
        b.iter(|| {
            rt.block_on(async {
                let table = PeerTable::new();
                for i in 0..100 {
                    let addr: SocketAddr = format!("127.0.0.1:{}", 9000 + i).parse().unwrap();
                    table
                        .upsert_discovered(make_agent_id(i), addr, make_pubkey(i))
                        .await;
                }
            });
        })
    });

    group.finish();
}

fn bench_get(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("peer_table_get");

    // Pre-populate a table with 50 peers
    let table = rt.block_on(async {
        let table = PeerTable::new();
        for i in 0..50 {
            let addr: SocketAddr = format!("127.0.0.1:{}", 9000 + i).parse().unwrap();
            table
                .upsert_discovered(make_agent_id(i), addr, make_pubkey(i))
                .await;
        }
        table
    });

    group.bench_function("hit", |b| {
        let id = make_agent_id(25);
        b.iter(|| {
            rt.block_on(async {
                let _ = table.get(black_box(&id)).await;
            })
        })
    });

    group.bench_function("miss", |b| {
        let id = make_agent_id(999);
        b.iter(|| {
            rt.block_on(async {
                let _ = table.get(black_box(&id)).await;
            })
        })
    });

    group.finish();
}

fn bench_list(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("peer_table_list");

    for count in [10, 50, 100] {
        let table = rt.block_on(async {
            let table = PeerTable::new();
            for i in 0..count {
                let addr: SocketAddr = format!("127.0.0.1:{}", 9000 + i).parse().unwrap();
                table
                    .upsert_discovered(make_agent_id(i), addr, make_pubkey(i))
                    .await;
            }
            table
        });

        group.bench_function(format!("{count}_peers"), |b| {
            b.iter(|| rt.block_on(async { table.list().await }))
        });
    }

    group.finish();
}

fn bench_remove_stale(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("peer_table_remove_stale");

    // All peers are fresh — nothing to remove
    let table = rt.block_on(async {
        let table = PeerTable::new();
        for i in 0..50 {
            let addr: SocketAddr = format!("127.0.0.1:{}", 9000 + i).parse().unwrap();
            table
                .upsert_discovered(make_agent_id(i), addr, make_pubkey(i))
                .await;
        }
        table
    });

    group.bench_function("50_peers_none_stale", |b| {
        b.iter(|| {
            rt.block_on(async { table.remove_stale(black_box(Duration::from_secs(60))).await })
        })
    });

    group.finish();
}

fn bench_set_status(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let table = rt.block_on(async {
        let table = PeerTable::new();
        for i in 0..50 {
            let addr: SocketAddr = format!("127.0.0.1:{}", 9000 + i).parse().unwrap();
            table
                .upsert_discovered(make_agent_id(i), addr, make_pubkey(i))
                .await;
        }
        table
    });

    let id = make_agent_id(25);
    c.bench_function("peer_table_set_connected", |b| {
        b.iter(|| {
            rt.block_on(async {
                table
                    .set_connected(black_box(&id), black_box(Some(1.5)))
                    .await;
            })
        })
    });
}

criterion_group!(
    benches,
    bench_upsert_discovered,
    bench_get,
    bench_list,
    bench_remove_stale,
    bench_set_status,
);
criterion_main!(benches);
