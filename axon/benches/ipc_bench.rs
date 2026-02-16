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
    use axon::ipc::IpcCommand;
    let mut group = c.benchmark_group("ipc_command_parse");

    let send_cmd = r#"{"cmd":"send","to":"ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"ping","payload":{}}"#;
    group.bench_function("send", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(send_cmd)).unwrap())
    });

    let peers_cmd = r#"{"cmd":"peers"}"#;
    group.bench_function("peers", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(peers_cmd)).unwrap())
    });

    let hello_cmd = r#"{"cmd":"hello","version":2}"#;
    group.bench_function("hello", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(hello_cmd)).unwrap())
    });

    let inbox_cmd = r#"{"cmd":"inbox","limit":50,"kinds":["query","notify"]}"#;
    group.bench_function("inbox", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(inbox_cmd)).unwrap())
    });

    let subscribe_cmd = r#"{"cmd":"subscribe","kinds":["query","delegate","notify"]}"#;
    group.bench_function("subscribe", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(subscribe_cmd)).unwrap())
    });

    group.finish();
}

fn bench_daemon_reply_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("daemon_reply_serialize");

    let reply_inbox = json!({
        "ok": true,
        "messages": [
            {
                "envelope": {
                    "v": 1, "id": "550e8400-e29b-41d4-a716-446655440000",
                    "from": "ed25519.aaaa", "to": "ed25519.bbbb",
                    "ts": 1700000000000u64, "kind": "notify",
                    "payload": {"topic": "test"}
                },
                "buffered_at": "2026-02-15T08:00:00.000Z"
            }
        ],
        "has_more": false
    });
    group.bench_function("inbox_reply", |b| {
        b.iter(|| serde_json::to_string(black_box(&reply_inbox)).unwrap())
    });

    let reply_hello = json!({
        "ok": true,
        "version": 2,
        "agent_id": "ed25519.a1b2c3d4e5f6a7b8",
        "features": ["auth", "buffer", "subscribe"]
    });
    group.bench_function("hello_reply", |b| {
        b.iter(|| serde_json::to_string(black_box(&reply_hello)).unwrap())
    });

    group.finish();
}

fn bench_ipc_inbound_event_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_v2_events");

    let envelope = make_envelope();

    // v2 InboundEvent serialization (what broadcast_inbound produces)
    group.bench_function("inbound_event", |b| {
        b.iter(|| {
            let event = json!({
                "event": "inbound",
                "replay": false,
                "seq": 42u64,
                "buffered_at_ms": 1700000000000u64,
                "envelope": black_box(&envelope),
            });
            serde_json::to_string(&event).unwrap()
        })
    });

    // v2 InboundEvent with Arc<str> fanout to 10 clients
    group.bench_function("inbound_event_fanout_10", |b| {
        b.iter(|| {
            let event = json!({
                "event": "inbound",
                "replay": false,
                "seq": 42u64,
                "buffered_at_ms": 1700000000000u64,
                "envelope": black_box(&envelope),
            });
            let line: std::sync::Arc<str> =
                std::sync::Arc::from(serde_json::to_string(&event).unwrap());
            for _ in 0..10 {
                let _cloned = line.clone();
            }
        })
    });

    group.finish();
}

fn bench_inbox_reply_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_v2_inbox");

    let envelope = make_envelope();

    // Inbox reply with 50 messages (default limit)
    let messages: Vec<serde_json::Value> = (0..50)
        .map(|i| {
            json!({
                "seq": i + 1,
                "buffered_at_ms": 1700000000000u64 + i * 1000,
                "envelope": &envelope,
            })
        })
        .collect();

    let reply = json!({
        "ok": true,
        "messages": messages,
        "next_seq": 50,
        "has_more": false,
        "req_id": "r1",
    });

    group.bench_function("inbox_50_messages", |b| {
        b.iter(|| serde_json::to_string(black_box(&reply)).unwrap())
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_ipc_serialize_inbound,
    bench_ipc_serialize_then_clone,
    bench_envelope_clone_vs_arc,
    bench_ipc_command_parse,
    bench_daemon_reply_serialize,
    bench_ipc_inbound_event_serialize,
    bench_inbox_reply_serialize,
);
criterion_main!(benches);
