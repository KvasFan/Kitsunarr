//! Indexer failure back-off after Sonarr's `IndexerStatusServiceFixture`.

use chrono::{Duration, Utc};
use kitsunarr_db::Database;
use kitsunarr_db::indexer_status::{self, Failure};

async fn db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    sqlx::query("INSERT INTO indexers (id, name, implementation, protocol, settings, created_at) VALUES (1, 'Nyaa', 'Torznab', 'torrent', '{}', '2026-11-01T00:00:00Z')")
        .execute(db.pool())
        .await
        .unwrap();
    (dir, db)
}

#[tokio::test]
async fn failures_escalate_and_successes_step_down() {
    let (_dir, db) = db().await;
    let pool = db.pool();
    let started = Utc::now() - Duration::hours(1);
    let now = Utc::now();
    // First failure: level 1, one minute.
    let s = indexer_status::record_failure(pool, 1, Failure::Failure, started, now)
        .await
        .unwrap();
    assert_eq!(s.escalation_level, 1);
    assert_eq!(
        s.disabled_till().unwrap().timestamp(),
        (now + Duration::minutes(1)).timestamp()
    );
    assert_eq!(indexer_status::blocked(pool, now).await.unwrap().len(), 1);
    // Second: level 2, five minutes; a connection failure does not escalate.
    let s = indexer_status::record_failure(pool, 1, Failure::Failure, started, now)
        .await
        .unwrap();
    assert_eq!(s.escalation_level, 2);
    let s = indexer_status::record_failure(pool, 1, Failure::Connection, started, now)
        .await
        .unwrap();
    assert_eq!(s.escalation_level, 2);
    // A rate limit of two hours raises the level until the period covers it (3 h).
    let s = indexer_status::record_failure(
        pool,
        1,
        Failure::RateLimited(Duration::hours(2)),
        started,
        now,
    )
    .await
    .unwrap();
    assert_eq!(s.escalation_level, 6);
    // Success: one level down, enabled again.
    indexer_status::record_success(pool, 1).await.unwrap();
    let s = indexer_status::get(pool, 1).await.unwrap();
    assert_eq!((s.escalation_level, s.disabled_till), (5, None));
    assert!(indexer_status::blocked(pool, now).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_startup_grace_period_caps_the_back_off() {
    let (_dir, db) = db().await;
    let pool = db.pool();
    let now = Utc::now();
    for _ in 0..5 {
        indexer_status::record_failure(pool, 1, Failure::Failure, now, now)
            .await
            .unwrap();
    }
    let s = indexer_status::get(pool, 1).await.unwrap();
    assert_eq!(s.escalation_level, 1, "no escalation just after start-up");
    assert!(s.disabled_till().unwrap() <= now + Duration::minutes(5));
}
