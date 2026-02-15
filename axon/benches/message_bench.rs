use criterion::{Criterion, black_box, criterion_group, criterion_main};
use serde_json::json;
use uuid::Uuid;

use axon::message::{Envelope, MessageKind, decode, encode};

fn make_small_envelope() -> Envelope {
    Envelope {
        v: 1,
        id: Uuid::new_v4(),
        from: "a".repeat(32).into(),
        to: "b".repeat(32).into(),
        ts: 1700000000000,
        kind: MessageKind::Ping,
        ref_id: None,
        payload: Envelope::raw_json(&json!({})),
    }
}

fn make_medium_envelope() -> Envelope {
    Envelope {
        v: 1,
        id: Uuid::new_v4(),
        from: "a".repeat(32).into(),
        to: "b".repeat(32).into(),
        ts: 1700000000000,
        kind: MessageKind::Query,
        ref_id: None,
        payload: Envelope::raw_json(&json!({
            "question": "What is the meaning of life, the universe, and everything?",
            "domain": "philosophy",
            "max_tokens": 1024,
            "deadline_ms": 30000
        })),
    }
}

fn make_large_envelope() -> Envelope {
    let big_data: Vec<String> = (0..100)
        .map(|i| format!("item_{i}: {}", "x".repeat(200)))
        .collect();
    Envelope {
        v: 1,
        id: Uuid::new_v4(),
        from: "a".repeat(32).into(),
        to: "b".repeat(32).into(),
        ts: 1700000000000,
        kind: MessageKind::Response,
        ref_id: Some(Uuid::new_v4()),
        payload: Envelope::raw_json(&json!({
            "data": big_data,
            "summary": "A large response payload for benchmarking purposes",
            "tokens_used": 4096,
            "truncated": false
        })),
    }
}

fn bench_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode");

    let small = make_small_envelope();
    group.bench_function("small_envelope", |b| {
        b.iter(|| encode(black_box(&small)).unwrap())
    });

    let medium = make_medium_envelope();
    group.bench_function("medium_envelope", |b| {
        b.iter(|| encode(black_box(&medium)).unwrap())
    });

    let large = make_large_envelope();
    group.bench_function("large_envelope", |b| {
        b.iter(|| encode(black_box(&large)).unwrap())
    });

    group.finish();
}

fn bench_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");

    let small_bytes = serde_json::to_vec(&make_small_envelope()).unwrap();
    group.bench_function("small_envelope", |b| {
        b.iter(|| decode(black_box(&small_bytes)).unwrap())
    });

    let medium_bytes = serde_json::to_vec(&make_medium_envelope()).unwrap();
    group.bench_function("medium_envelope", |b| {
        b.iter(|| decode(black_box(&medium_bytes)).unwrap())
    });

    let large_bytes = serde_json::to_vec(&make_large_envelope()).unwrap();
    group.bench_function("large_envelope", |b| {
        b.iter(|| decode(black_box(&large_bytes)).unwrap())
    });

    group.finish();
}

fn bench_encode_decode_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");

    let small = make_small_envelope();
    group.bench_function("small_envelope", |b| {
        b.iter(|| {
            let encoded = encode(black_box(&small)).unwrap();
            let _decoded = decode(&encoded[4..]).unwrap();
        })
    });

    let medium = make_medium_envelope();
    group.bench_function("medium_envelope", |b| {
        b.iter(|| {
            let encoded = encode(black_box(&medium)).unwrap();
            let _decoded = decode(&encoded[4..]).unwrap();
        })
    });

    let large = make_large_envelope();
    group.bench_function("large_envelope", |b| {
        b.iter(|| {
            let encoded = encode(black_box(&large)).unwrap();
            let _decoded = decode(&encoded[4..]).unwrap();
        })
    });

    group.finish();
}

fn bench_envelope_validate(c: &mut Criterion) {
    let envelope = make_medium_envelope();
    c.bench_function("envelope_validate", |b| {
        b.iter(|| black_box(&envelope).validate().unwrap())
    });
}

fn bench_envelope_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("envelope_clone");

    let small = make_small_envelope();
    group.bench_function("small", |b| b.iter(|| black_box(&small).clone()));

    let medium = make_medium_envelope();
    group.bench_function("medium", |b| b.iter(|| black_box(&medium).clone()));

    let large = make_large_envelope();
    group.bench_function("large", |b| b.iter(|| black_box(&large).clone()));

    group.finish();
}

fn bench_serde_json_to_vec(c: &mut Criterion) {
    let mut group = c.benchmark_group("serde_json_to_vec");

    let small = make_small_envelope();
    group.bench_function("small", |b| {
        b.iter(|| serde_json::to_vec(black_box(&small)).unwrap())
    });

    let large = make_large_envelope();
    group.bench_function("large", |b| {
        b.iter(|| serde_json::to_vec(black_box(&large)).unwrap())
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_encode,
    bench_decode,
    bench_encode_decode_roundtrip,
    bench_envelope_validate,
    bench_envelope_clone,
    bench_serde_json_to_vec,
);
criterion_main!(benches);
