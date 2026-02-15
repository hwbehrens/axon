use std::time::{Duration, Instant};

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use uuid::Uuid;

// ReplayCache is pub(crate), so we reimplement a minimal version here
// that mirrors the same data structure and algorithm for benchmarking.
use std::collections::HashMap;

struct ReplayCache {
    ttl: Duration,
    seen: HashMap<Uuid, Instant>,
}

impl ReplayCache {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            seen: HashMap::new(),
        }
    }

    fn is_replay(&mut self, id: Uuid, now: Instant) -> bool {
        self.seen
            .retain(|_, ts| now.saturating_duration_since(*ts) <= self.ttl);
        if self.seen.contains_key(&id) {
            return true;
        }
        self.seen.insert(id, now);
        false
    }
}

fn bench_is_replay_empty(c: &mut Criterion) {
    c.bench_function("replay_cache_is_replay_empty", |b| {
        let mut cache = ReplayCache::new(Duration::from_secs(300));
        let now = Instant::now();
        b.iter(|| {
            let id = Uuid::new_v4();
            cache.is_replay(black_box(id), black_box(now));
        })
    });
}

fn bench_is_replay_with_entries(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_cache_is_replay");

    for count in [100, 1000, 5000] {
        group.bench_function(format!("{count}_entries"), |b| {
            let now = Instant::now();
            let mut cache = ReplayCache::new(Duration::from_secs(300));
            for _ in 0..count {
                cache.is_replay(Uuid::new_v4(), now);
            }

            b.iter(|| {
                let id = Uuid::new_v4();
                cache.is_replay(black_box(id), black_box(now));
            })
        });
    }

    group.finish();
}

fn bench_is_replay_duplicate_check(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_cache_duplicate_check");

    for count in [100, 1000, 5000] {
        group.bench_function(format!("{count}_entries"), |b| {
            let now = Instant::now();
            let mut cache = ReplayCache::new(Duration::from_secs(300));
            let known_id = Uuid::new_v4();
            cache.is_replay(known_id, now);
            for _ in 0..count {
                cache.is_replay(Uuid::new_v4(), now);
            }

            b.iter(|| {
                cache.is_replay(black_box(known_id), black_box(now));
            })
        });
    }

    group.finish();
}

fn bench_is_replay_with_expiration(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_cache_expiration");

    // All entries are expired — retain removes everything
    for count in [100, 1000, 5000] {
        group.bench_function(format!("{count}_expired_entries"), |b| {
            b.iter_custom(|iters| {
                let ttl = Duration::from_millis(1);
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let past = Instant::now();
                    let mut cache = ReplayCache::new(ttl);
                    for _ in 0..count {
                        cache.is_replay(Uuid::new_v4(), past);
                    }
                    let now = Instant::now() + Duration::from_millis(10);
                    let start = Instant::now();
                    cache.is_replay(Uuid::new_v4(), now);
                    total += start.elapsed();
                }
                total
            })
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_is_replay_empty,
    bench_is_replay_with_entries,
    bench_is_replay_duplicate_check,
    bench_is_replay_with_expiration,
);
criterion_main!(benches);
