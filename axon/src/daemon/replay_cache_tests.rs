use super::*;

#[tokio::test]
async fn replay_cache_marks_duplicates() {
    let cache = ReplayCache::new(Duration::from_secs(10), 1000);
    let id = uuid::Uuid::new_v4();
    let now = Instant::now();

    assert!(!cache.is_replay(id, now).await);
    assert!(cache.is_replay(id, now).await);
}

#[tokio::test]
async fn replay_cache_expires_old_entries() {
    let cache = ReplayCache::new(Duration::from_secs(1), 1000);
    let id = uuid::Uuid::new_v4();
    let now = Instant::now();

    assert!(!cache.is_replay(id, now).await);
    assert!(cache.is_replay(id, now).await);
    assert!(!cache.is_replay(id, now + Duration::from_secs(2)).await);
}

#[tokio::test]
async fn replay_cache_different_ids_not_duplicates() {
    let cache = ReplayCache::new(Duration::from_secs(10), 1000);
    let now = Instant::now();

    assert!(!cache.is_replay(uuid::Uuid::new_v4(), now).await);
    assert!(!cache.is_replay(uuid::Uuid::new_v4(), now).await);
}

#[test]
fn clock_validation_would_catch_zero() {
    let ms = crate::message::now_millis();
    assert!(
        ms > 0,
        "system clock returned 0 — test environment has invalid clock"
    );
}

// =========================================================================
// Mutation-coverage: save/load persistence
// =========================================================================

#[tokio::test]
async fn save_and_reload_persists_entries() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("replay.json");
    let ttl = Duration::from_secs(60);

    let id1 = uuid::Uuid::new_v4();
    let id2 = uuid::Uuid::new_v4();

    let cache = ReplayCache::new(ttl, 1000);
    let now = Instant::now();
    cache.is_replay(id1, now).await;
    cache.is_replay(id2, now).await;
    cache.save(&path).await.expect("save");

    let cache2 = ReplayCache::load(&path, ttl, 1000);
    assert!(
        cache2.is_replay(id1, Instant::now()).await,
        "id1 should be detected as replay after reload"
    );
    assert!(
        cache2.is_replay(id2, Instant::now()).await,
        "id2 should be detected as replay after reload"
    );
}

#[tokio::test]
async fn load_preserves_recent_entries() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("replay.json");
    let ttl = Duration::from_secs(10);

    let id = uuid::Uuid::new_v4();

    let cache = ReplayCache::new(ttl, 1000);
    let now = Instant::now();
    cache.is_replay(id, now).await;
    cache.save(&path).await.expect("save");

    // Load with same TTL — entry was seen ~0s ago, well within 10s TTL
    let cache2 = ReplayCache::load(&path, ttl, 1000);
    assert!(
        cache2.is_replay(id, Instant::now()).await,
        "recently-seen entry should be preserved by load, not discarded"
    );
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

proptest! {
    #[test]
    fn unique_ids_never_marked_replay(count in 1usize..100) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let cache = ReplayCache::new(Duration::from_secs(60), 1000);
            let now = Instant::now();
            for _ in 0..count {
                let id = uuid::Uuid::new_v4();
                prop_assert!(!cache.is_replay(id, now).await);
            }
            Ok(())
        })?;
    }

    #[test]
    fn duplicate_always_detected_within_ttl(offset_ms in 0u64..59_000) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let cache = ReplayCache::new(Duration::from_secs(60), 1000);
            let now = Instant::now();
            let id = uuid::Uuid::new_v4();
            cache.is_replay(id, now).await;
            let later = now + Duration::from_millis(offset_ms);
            prop_assert!(cache.is_replay(id, later).await);
            Ok(())
        })?;
    }

    #[test]
    fn expired_entries_reinsertable(ttl_secs in 1u64..10u64) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let cache = ReplayCache::new(Duration::from_secs(ttl_secs), 1000);
            let now = Instant::now();
            let id = uuid::Uuid::new_v4();
            cache.is_replay(id, now).await;
            let expired = now + Duration::from_secs(ttl_secs + 1);
            prop_assert!(!cache.is_replay(id, expired).await);
            Ok(())
        })?;
    }
}
