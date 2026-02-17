use std::sync::Arc;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use serde_json::json;
use uuid::Uuid;

use axon::ipc::{DaemonReply, IpcCommand, IpcErrorCode, PeerSummary, WhoamiInfo};
use axon::message::{AgentId, Envelope, MessageKind};

fn make_envelope() -> Envelope {
    Envelope {
        id: Uuid::new_v4(),
        kind: MessageKind::Message,
        ref_id: None,
        payload: Envelope::raw_json(&json!({
            "topic": "build.progress",
            "data": {"step": 3, "total": 10, "message": "Compiling module xyz"},
            "importance": "medium"
        })),
        from: Some(AgentId::from(format!("ed25519.{}", "a".repeat(32)))),
        to: Some(AgentId::from(format!("ed25519.{}", "b".repeat(32)))),
    }
}

fn bench_ipc_command_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_command_parse");

    let send_request = r#"{"cmd":"send","to":"ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"request","payload":{}}"#;
    group.bench_function("send_request", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(send_request)).unwrap())
    });

    let send_message = r#"{"cmd":"send","to":"ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"message","payload":{}}"#;
    group.bench_function("send_message", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(send_message)).unwrap())
    });

    let peers_cmd = r#"{"cmd":"peers"}"#;
    group.bench_function("peers", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(peers_cmd)).unwrap())
    });

    let status_cmd = r#"{"cmd":"status"}"#;
    group.bench_function("status", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(status_cmd)).unwrap())
    });

    let whoami_cmd = r#"{"cmd":"whoami"}"#;
    group.bench_function("whoami", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(whoami_cmd)).unwrap())
    });

    let invalid_send_kind = r#"{"cmd":"send","to":"ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"response","payload":{}}"#;
    group.bench_function("invalid_send_kind", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(invalid_send_kind)).is_err())
    });

    let unknown_cmd = r#"{"cmd":"nonexistent"}"#;
    group.bench_function("unknown_cmd", |b| {
        b.iter(|| serde_json::from_str::<IpcCommand>(black_box(unknown_cmd)).is_err())
    });

    group.finish();
}

fn bench_daemon_reply_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("daemon_reply_serialize");

    let send_ok = DaemonReply::SendOk {
        ok: true,
        msg_id: Uuid::new_v4(),
        req_id: Some("req-1".to_string()),
        response: None,
    };
    group.bench_function("send_ok", |b| {
        b.iter(|| serde_json::to_string(black_box(&send_ok)).unwrap())
    });

    let request = Envelope::new(
        AgentId::from(format!("ed25519.{}", "a".repeat(32))),
        AgentId::from(format!("ed25519.{}", "b".repeat(32))),
        MessageKind::Request,
        json!({"question": "hello?"}),
    );
    let response = Envelope::response_to(
        &request,
        AgentId::from(format!("ed25519.{}", "b".repeat(32))),
        MessageKind::Response,
        json!({"answer": "hi"}),
    );
    let send_ok_with_response = DaemonReply::SendOk {
        ok: true,
        msg_id: request.id,
        req_id: Some("req-2".to_string()),
        response: Some(response),
    };
    group.bench_function("send_ok_with_response", |b| {
        b.iter(|| serde_json::to_string(black_box(&send_ok_with_response)).unwrap())
    });

    let peers = DaemonReply::Peers {
        ok: true,
        peers: vec![PeerSummary {
            id: format!("ed25519.{}", "a".repeat(32)),
            addr: "127.0.0.1:7100".to_string(),
            status: "connected".to_string(),
            rtt_ms: Some(1.2),
            source: "static".to_string(),
        }],
        req_id: Some("req-3".to_string()),
    };
    group.bench_function("peers", |b| {
        b.iter(|| serde_json::to_string(black_box(&peers)).unwrap())
    });

    let status = DaemonReply::Status {
        ok: true,
        uptime_secs: 99,
        peers_connected: 2,
        messages_sent: 10,
        messages_received: 5,
        req_id: Some("req-4".to_string()),
    };
    group.bench_function("status", |b| {
        b.iter(|| serde_json::to_string(black_box(&status)).unwrap())
    });

    let whoami = DaemonReply::Whoami {
        ok: true,
        info: WhoamiInfo {
            agent_id: format!("ed25519.{}", "a".repeat(32)),
            public_key: "cHVia2V5".to_string(),
            name: Some("bench-agent".to_string()),
            version: "0.3.0".to_string(),
            uptime_secs: 321,
        },
        req_id: Some("req-5".to_string()),
    };
    group.bench_function("whoami", |b| {
        b.iter(|| serde_json::to_string(black_box(&whoami)).unwrap())
    });

    let err = DaemonReply::Error {
        ok: false,
        error: IpcErrorCode::InvalidCommand,
        message: IpcErrorCode::InvalidCommand.message(),
        req_id: Some("req-6".to_string()),
    };
    group.bench_function("error", |b| {
        b.iter(|| serde_json::to_string(black_box(&err)).unwrap())
    });

    let inbound = DaemonReply::InboundEvent {
        event: "inbound",
        from: format!("ed25519.{}", "a".repeat(32)),
        envelope: make_envelope(),
    };
    group.bench_function("inbound_event", |b| {
        b.iter(|| serde_json::to_string(black_box(&inbound)).unwrap())
    });

    group.finish();
}

fn bench_ipc_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_fanout");

    let inbound = DaemonReply::InboundEvent {
        event: "inbound",
        from: format!("ed25519.{}", "a".repeat(32)),
        envelope: make_envelope(),
    };

    for clients in [1, 5, 10] {
        group.bench_function(format!("{clients}_clients_string_clone"), |b| {
            b.iter(|| {
                let line = serde_json::to_string(black_box(&inbound)).unwrap();
                for _ in 0..clients {
                    let _cloned = line.clone();
                }
            })
        });
    }

    for clients in [1, 5, 10] {
        group.bench_function(format!("{clients}_clients_arc_clone"), |b| {
            b.iter(|| {
                let line: Arc<str> = Arc::from(serde_json::to_string(black_box(&inbound)).unwrap());
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

    let arc_envelope = Arc::new(make_envelope());
    group.bench_function("arc_clone", |b| b.iter(|| black_box(&arc_envelope).clone()));

    group.finish();
}

criterion_group!(
    benches,
    bench_ipc_command_parse,
    bench_daemon_reply_serialize,
    bench_ipc_fanout,
    bench_envelope_clone_vs_arc,
);
criterion_main!(benches);
