//! First-run defaults. Every step is idempotent: existing user data is never overwritten.

use kitsunarr_domain::api::settings::{GeneralSettings, MediaManagementSettings, NamingSettings};

use crate::{Database, DbResult, quality, settings, tasks};

/// Built-in scheduled tasks: (name, default interval in minutes).
pub const DEFAULT_TASKS: &[(&str, i64)] = &[
    ("Housekeeping", 24 * 60),
    ("HealthCheck", 6 * 60),
    ("Backup", 7 * 24 * 60),
    ("CleanupSessions", 60),
    ("RefreshMetadata", 12 * 60),
    ("RssSync", 15),
];

pub async fn seed_defaults(db: &Database) -> DbResult<()> {
    let mut tx = db.pool().begin().await?;
    let defs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quality_definitions")
        .fetch_one(&mut *tx)
        .await?;
    if defs == 0 {
        for d in quality::default_definitions() {
            quality::upsert_definition(&mut *tx, &d).await?;
        }
    }
    let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quality_profiles")
        .fetch_one(&mut *tx)
        .await?;
    if profiles == 0 {
        for p in quality::default_profiles() {
            quality::create_profile(&mut *tx, &p).await?;
        }
    }
    // Sonarr's default delay profile: no delay, last in order, for every series.
    let delays: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delay_profiles")
        .fetch_one(&mut *tx)
        .await?;
    if delays == 0 {
        crate::rules::create_delay_profile(
            &mut *tx,
            &crate::rules::DelayProfile {
                id: 0,
                enable_usenet: true,
                enable_torrent: true,
                preferred_protocol: "usenet".into(),
                usenet_delay: 0,
                torrent_delay: 0,
                bypass_if_highest_quality: true,
                bypass_if_above_custom_format_score: false,
                minimum_custom_format_score: 0,
                sort_order: i64::from(i32::MAX),
                tags: Vec::new(),
            },
        )
        .await?;
    }
    settings::set_if_absent(&mut *tx, settings::KEY_NAMING, &NamingSettings::default()).await?;
    settings::set_if_absent(
        &mut *tx,
        settings::KEY_MEDIA_MANAGEMENT,
        &MediaManagementSettings::default(),
    )
    .await?;
    settings::set_if_absent(&mut *tx, settings::KEY_GENERAL, &GeneralSettings::default()).await?;
    for (name, minutes) in DEFAULT_TASKS {
        tasks::ensure(&mut *tx, name, *minutes).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Returns the stored API key, creating one with `generate` on first start.
pub async fn ensure_api_key(db: &Database, generate: impl FnOnce() -> String) -> DbResult<String> {
    if let Some(key) = settings::get::<String>(db.pool(), settings::KEY_API_KEY).await? {
        return Ok(key);
    }
    let key = generate();
    settings::set_if_absent(db.pool(), settings::KEY_API_KEY, &key).await?;
    // Re-read in case another task won the race.
    Ok(settings::get::<String>(db.pool(), settings::KEY_API_KEY)
        .await?
        .unwrap_or(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn seeding_is_idempotent() {
        let db = crate::test_support::db().await;
        seed_defaults(&db).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quality_profiles")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(n, 6);
        let k1 = ensure_api_key(&db, || "first".into()).await.unwrap();
        let k2 = ensure_api_key(&db, || "second".into()).await.unwrap();
        assert_eq!((k1.as_str(), k2.as_str()), ("first", "first"));
    }
}
