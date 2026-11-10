//! Wanted episodes: Sonarr's `EpisodesWithoutFiles` (Missing) and `EpisodesWhereCutoffUnmet`.

use kitsunarr_domain::api::wanted::WantedEpisode;
use kitsunarr_domain::{Page, PageRequest, Quality, SortDirection};
use sqlx::FromRow;

use crate::{Database, DbResult};

/// The qualities below a profile's cutoff (`QualitiesBelowCutoff`).
#[derive(Debug, Clone)]
pub struct BelowCutoff {
    pub profile_id: i64,
    pub quality_ids: Vec<i32>,
}

/// Which wanted list, with what it needs.
#[derive(Debug, Clone, Copy)]
pub enum Wanted<'a> {
    /// Aired (air date plus runtime in the past) and without a file. `now` is
    /// `YYYY-MM-DD HH:MM:SS` in UTC.
    Missing { now: &'a str },
    /// With a file whose quality is below the series profile's cutoff.
    CutoffUnmet { below: &'a [BelowCutoff] },
}

#[derive(FromRow)]
struct Row {
    episode_id: i64,
    series_id: i64,
    series_title: String,
    season_number: i64,
    episode_number: i64,
    absolute_episode_number: Option<i64>,
    title: Option<String>,
    air_date_utc: Option<String>,
    monitored: bool,
    last_search_at: Option<String>,
    quality_id: Option<i64>,
}

/// Appends the `WHERE` condition. `false` when the list is empty by definition (no profile
/// has qualities below its cutoff).
fn push_filter(
    q: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
    list: Wanted<'_>,
    monitored: bool,
) -> bool {
    let groups: Vec<&BelowCutoff> = match list {
        Wanted::CutoffUnmet { below } => {
            below.iter().filter(|b| !b.quality_ids.is_empty()).collect()
        }
        Wanted::Missing { .. } => Vec::new(),
    };
    if matches!(list, Wanted::CutoffUnmet { .. }) && groups.is_empty() {
        return false;
    }
    q.push(if monitored {
        " WHERE e.monitored = 1 AND s.monitored = 1"
    } else {
        " WHERE (e.monitored = 0 OR s.monitored = 0)"
    });
    match list {
        Wanted::Missing { now } => {
            q.push(
                " AND e.episode_file_id IS NULL AND e.air_date_utc IS NOT NULL \
                 AND datetime(strftime('%s', e.air_date_utc) + COALESCE(s.runtime, 0) * 60, 'unixepoch') <= ",
            );
            q.push_bind(now.to_string());
        }
        Wanted::CutoffUnmet { .. } => {
            q.push(" AND e.episode_file_id IS NOT NULL AND (");
            for (i, b) in groups.iter().enumerate() {
                if i > 0 {
                    q.push(" OR ");
                }
                q.push("(s.quality_profile_id = ");
                q.push_bind(b.profile_id);
                q.push(" AND f.quality_id IN (");
                let mut sep = q.separated(", ");
                for id in &b.quality_ids {
                    sep.push_bind(*id);
                }
                q.push("))");
            }
            q.push(")");
        }
    }
    true
}

const FROM: &str = " FROM episodes e JOIN series s ON s.id = e.series_id \
     LEFT JOIN episode_files f ON f.id = e.episode_file_id";

/// One page of a wanted list, by air date (newest first unless ascending is asked for).
pub async fn page(
    db: &Database,
    list: Wanted<'_>,
    monitored: bool,
    paging: &PageRequest,
) -> DbResult<Page<WantedEpisode>> {
    let empty = Page {
        page: paging.page(),
        page_size: paging.page_size(),
        total_records: 0,
        records: Vec::new(),
    };
    let mut count = sqlx::QueryBuilder::new("SELECT COUNT(*)");
    count.push(FROM);
    if !push_filter(&mut count, list, monitored) {
        return Ok(empty);
    }
    let total: i64 = count.build_query_scalar().fetch_one(db.pool()).await?;
    let order = if paging.sort_direction == Some(SortDirection::Asc) {
        "ASC"
    } else {
        "DESC"
    };
    let mut q = sqlx::QueryBuilder::new(
        "SELECT e.id AS episode_id, e.series_id, s.title AS series_title, e.season_number, \
           e.episode_number, e.absolute_episode_number, e.title, e.air_date_utc, e.monitored, \
           e.last_search_at, f.quality_id",
    );
    q.push(FROM);
    push_filter(&mut q, list, monitored);
    q.push(format!(
        " ORDER BY e.air_date_utc {order}, e.id {order} LIMIT "
    ));
    q.push_bind(i64::from(paging.page_size()));
    q.push(" OFFSET ");
    q.push_bind(paging.offset() as i64);
    let rows: Vec<Row> = q.build_query_as().fetch_all(db.pool()).await?;
    Ok(Page {
        records: rows
            .into_iter()
            .map(|r| WantedEpisode {
                episode_id: r.episode_id,
                series_id: r.series_id,
                series_title: r.series_title,
                season_number: r.season_number as i32,
                episode_number: r.episode_number as i32,
                absolute_episode_number: r.absolute_episode_number.map(|a| a as i32),
                title: r.title,
                air_date_utc: r.air_date_utc,
                monitored: r.monitored,
                last_search_at: r.last_search_at,
                quality_name: r
                    .quality_id
                    .and_then(|q| Quality::by_id(q as i32))
                    .map(|q| q.name.to_string()),
            })
            .collect(),
        total_records: total as u64,
        ..empty
    })
}

/// An episode to search for.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SearchTarget {
    pub episode_id: i64,
    pub series_id: i64,
    pub season_number: i64,
    pub last_search_at: Option<String>,
}

/// Every episode of a wanted list, by id (`MissingEpisodeSearch`, `CutoffUnmetEpisodeSearch`).
pub async fn all(db: &Database, list: Wanted<'_>, monitored: bool) -> DbResult<Vec<SearchTarget>> {
    let mut q = sqlx::QueryBuilder::new(
        "SELECT e.id AS episode_id, e.series_id, e.season_number, e.last_search_at",
    );
    q.push(FROM);
    if !push_filter(&mut q, list, monitored) {
        return Ok(Vec::new());
    }
    q.push(" ORDER BY e.id");
    Ok(q.build_query_as().fetch_all(db.pool()).await?)
}

/// The given episodes, for a search of chosen episodes (`EpisodeSearch`).
pub async fn targets(db: &Database, episode_ids: &[i64]) -> DbResult<Vec<SearchTarget>> {
    if episode_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = sqlx::QueryBuilder::new(
        "SELECT id AS episode_id, series_id, season_number, last_search_at FROM episodes WHERE id IN (",
    );
    let mut sep = q.separated(", ");
    for id in episode_ids {
        sep.push_bind(*id);
    }
    q.push(") ORDER BY id");
    Ok(q.build_query_as().fetch_all(db.pool()).await?)
}

/// `UpdateLastSearchTime`.
pub async fn set_last_search(db: &Database, episode_ids: &[i64], when: &str) -> DbResult<()> {
    if episode_ids.is_empty() {
        return Ok(());
    }
    let mut q = sqlx::QueryBuilder::new("UPDATE episodes SET last_search_at = ");
    q.push_bind(when);
    q.push(" WHERE id IN (");
    let mut sep = q.separated(", ");
    for id in episode_ids {
        sep.push_bind(*id);
    }
    q.push(")");
    q.build().execute(db.pool()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::series::{self, tests::insert_sample};
    use kitsunarr_domain::api::library::SeriesUpdate;

    #[tokio::test]
    async fn missing_cutoff_unmet_and_search_targets() {
        let db = crate::test_support::db().await;
        // Episode 1 has a Bluray-1080p file; episodes 2-4 aired on 2008-01-22..24 at 02:00 UTC
        // and run 47 minutes.
        let id = insert_sample(&db, "Breaking Bad", "81189", 4).await;
        let paging = PageRequest::default();
        let missing = |now| Wanted::Missing { now };

        let p = page(&db, missing("2026-11-10 00:00:00"), true, &paging)
            .await
            .unwrap();
        assert_eq!(p.total_records, 3);
        let numbers: Vec<i32> = p.records.iter().map(|r| r.episode_number).collect();
        assert_eq!(numbers, [4, 3, 2], "newest first");
        assert_eq!(p.records[0].series_title, "Breaking Bad");
        assert_eq!(p.records[0].quality_name, None);
        let oldest_first = PageRequest {
            sort_direction: Some(SortDirection::Asc),
            page_size: Some(2),
            ..Default::default()
        };
        let p = page(&db, missing("2026-11-10 00:00:00"), true, &oldest_first)
            .await
            .unwrap();
        assert_eq!((p.total_records, p.records.len()), (3, 2));
        assert_eq!(p.records[0].episode_number, 2);

        // An episode is missing once it has aired: air date plus runtime.
        let at = |now| page(&db, missing(now), true, &paging);
        assert_eq!(at("2008-01-22 02:46:00").await.unwrap().total_records, 0);
        assert_eq!(at("2008-01-22 02:47:00").await.unwrap().total_records, 1);

        // The unmonitored list holds episodes of unmonitored series (or unmonitored episodes).
        assert_eq!(
            page(&db, missing("2026-11-10 00:00:00"), false, &paging)
                .await
                .unwrap()
                .total_records,
            0
        );
        series::apply_update(
            &db,
            id,
            &SeriesUpdate {
                monitored: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(at("2026-11-10 00:00:00").await.unwrap().total_records, 0);
        assert_eq!(
            page(&db, missing("2026-11-10 00:00:00"), false, &paging)
                .await
                .unwrap()
                .total_records,
            3
        );
        series::apply_update(
            &db,
            id,
            &SeriesUpdate {
                monitored: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Cutoff unmet: files whose quality is below the profile's cutoff.
        let below = [BelowCutoff {
            profile_id: 1,
            quality_ids: vec![4, 7],
        }];
        let p = page(&db, Wanted::CutoffUnmet { below: &below }, true, &paging)
            .await
            .unwrap();
        assert_eq!(p.total_records, 1);
        assert_eq!(p.records[0].episode_number, 1);
        assert_eq!(p.records[0].quality_name.as_deref(), Some("Bluray-1080p"));
        let other_profile = [BelowCutoff {
            profile_id: 2,
            quality_ids: vec![7],
        }];
        let none = page(
            &db,
            Wanted::CutoffUnmet {
                below: &other_profile,
            },
            true,
            &paging,
        );
        assert_eq!(none.await.unwrap().total_records, 0);
        let none = page(&db, Wanted::CutoffUnmet { below: &[] }, true, &paging);
        assert_eq!(none.await.unwrap().total_records, 0);

        // Search targets, and the last search time.
        let all_missing = all(&db, missing("2026-11-10 00:00:00"), true)
            .await
            .unwrap();
        assert_eq!(all_missing.len(), 3);
        assert!(
            all_missing
                .iter()
                .all(|t| t.series_id == id && t.season_number == 1)
        );
        let ids: Vec<i64> = all_missing.iter().map(|t| t.episode_id).collect();
        set_last_search(&db, &ids[..2], "2026-11-10T10:00:00Z")
            .await
            .unwrap();
        let chosen = targets(&db, &ids).await.unwrap();
        assert_eq!(
            chosen
                .iter()
                .map(|t| t.last_search_at.as_deref())
                .collect::<Vec<_>>(),
            [
                Some("2026-11-10T10:00:00Z"),
                Some("2026-11-10T10:00:00Z"),
                None
            ]
        );
        assert!(targets(&db, &[]).await.unwrap().is_empty());
    }
}
