use criterion::{Criterion, black_box, criterion_group, criterion_main};
use serde_json::json;
use uuid::Uuid;

use axon::message::{Envelope, MessageKind};

fn make_envelope() -> Envelope {
    Envelope {
        v: 1,
        id: Uuid::new_v4(),
        from: format!("ed25519.{}", "a".repeat(32)).into(),
        to: format!("ed25519.{}", "b".repeat(32)).into(),
        ts: 1700000000000,
        kind: MessageKind::Notify,
        ref_id: None,
        payload: Envelope::raw_json(&json!({
            "topic": "build.progress",
            "data": {"step": 3, "total": 10, "message": "Compiling module xyz"},
            "importance": "medium"
        })),
    }
}

fn bench_ipc_serialize_inbound(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_serialize");

    let envelope = make_envelope();
    // This mirrors what broadcast_inbound does: wrap in DaemonReply::Inbound and serialize
    group.bench_function("inbound_envelope", |b| {
        b.iter(|| {
            let msg = json!({
                "inbound": true,
                "envelope": black_box(&envelope),
            });
            serde_json::to_string(&msg).unwrap()
        })
    });

    group.finish();
}

fn bench_ipc_serialize_then_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_fanout");

    let envelope = make_envelope();

    // Simulate fanout to N clients: serialize once, clone N times
    for clients in [1, 5, 10] {
        group.bench_function(format!("{clients}_clients_string_clone"), |b| {
            b.iter(|| {
                let msg = json!({
                    "inbound": true,
                    "envelope": black_box(&envelope),
                });
                let line = serde_json::to_string(&msg).unwrap();
                for _ in 0..clients {
                    let _cloned = line.clone();
                }
            })
        });
    }

    // Compare: Arc<str> — serialize once, Arc clone is cheap
    for clients in [1, 5, 10] {
        group.bench_function(format!("{clients}_clients_arc_clone"), |b| {
            b.iter(|| {
                let msg = json!({
                    "inbound": true,
                    "envelope": black_box(&envelope),
                });
                let line: std::sync::Arc<str> =
                    std::sync::Arc::from(serde_json::to_string(&msg).unwrap());
                for _ in 0..clients {
                    let _cloned = line.clone();
                }
            })
        });
    }

    group.finish();
}

fn bench_envelope_clone_vs_arc(c: &mut Criterion) {
    let mut group = c.benchmark_group("envelope_clone_vs_arc");

    let envelope = make_envelope();

    group.bench_function("direct_clone", |b| b.iter(|| black_box(&envelope).clone()));

    let arc_envelope = std::sync::Arc::new(make_envelope());
    group.bench_function("arc_clone", |b| b.iter(|| black_box(&arc_envelope).clone()));

    group.finish();
}

fn bench_ipc_command_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_command_parse");

    let send_cmd =
        r#"{"cmd":"send","to":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"ping","payload":{}}"#;
    group.bench_function("send", |b| {
        b.iter(|| serde_json::from_str::<serde_json::Value>(black_box(send_cmd)).unwrap())
    });

    let peers_cmd = r#"{"cmd":"peers"}"#;
    group.bench_function("peers", |b| {
        b.iter(|| serde_json::from_str::<serde_json::Value>(black_box(peers_cmd)).unwrap())
    });

    let status_cmd = r#"{"cmd":"status"}"#;
    group.bench_function("status", |b| {
        b.iter(|| serde_json::from_str::<serde_json::Value>(black_box(status_cmd)).unwrap())
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_ipc_serialize_inbound,
    bench_ipc_serialize_then_clone,
    bench_envelope_clone_vs_arc,
    bench_ipc_command_parse,
);
criterion_main!(benches);
