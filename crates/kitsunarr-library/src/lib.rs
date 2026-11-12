//! Library services: add titles from metadata providers and refresh them.
//!
//! Adding never touches the filesystem: the series/movie folder path is derived from the root
//! folder and the naming settings, and folders are created by the import pipeline when files
//! arrive. Refreshing updates provider-owned fields only.

pub mod decision_input;
pub mod downloads;
pub mod failed;
pub mod file_names;
pub mod import;

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use kitsunarr_db::library::{movies, series};
use kitsunarr_db::{Database, DbError, root_folders, settings};
use kitsunarr_domain::api::metadata::{
    AddTitleRequest, EntryKind, LibraryRef, MonitorOption, RefreshResult,
};
use kitsunarr_domain::api::settings::NamingSettings;
use kitsunarr_domain::{
    MediaKind, MetadataProvider, MinimumAvailability, MonitorNewItems, MovieStatus, SeriesStatus,
    SeriesType, naming,
};
use kitsunarr_metadata::{EpisodeInfo, Metadata, MetadataError, SeriesInfo};

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    /// A request field is invalid; `field` is the camelCase request field name.
    #[error("{message}")]
    Invalid {
        field: &'static str,
        message: String,
    },
    #[error("already in the library")]
    Exists(LibraryRef),
}

impl LibraryError {
    fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        LibraryError::Invalid {
            field,
            message: message.into(),
        }
    }
}

impl From<sqlx::Error> for LibraryError {
    fn from(e: sqlx::Error) -> Self {
        LibraryError::Db(DbError::Sqlx(e))
    }
}

pub type LibraryResult<T> = Result<T, LibraryError>;

/// Finds a library entry matching the catalogue identity or any of its external ids.
pub async fn find_existing(
    db: &Database,
    provider: MetadataProvider,
    id: &str,
    external: &[kitsunarr_domain::ExternalId],
) -> LibraryResult<Option<LibraryRef>> {
    let mut conn = db.pool().acquire().await?;
    if let Some(s) = series::find_by_metadata(&mut conn, provider, id).await? {
        return Ok(Some(LibraryRef {
            media_type: "series".into(),
            id: s,
        }));
    }
    if let Some(m) = movies::find_by_metadata(&mut conn, provider, id).await? {
        return Ok(Some(LibraryRef {
            media_type: "movie".into(),
            id: m,
        }));
    }
    for ext in external {
        if let Some(s) = series::find_by_external_id(&mut conn, ext.kind, &ext.value).await? {
            return Ok(Some(LibraryRef {
                media_type: "series".into(),
                id: s,
            }));
        }
        if let Some(m) = movies::find_by_external_id(&mut conn, ext.kind, &ext.value).await? {
            return Ok(Some(LibraryRef {
                media_type: "movie".into(),
                id: m,
            }));
        }
    }
    Ok(None)
}

/// Whether each episode has aired: from its own air date or, when the catalogue has none,
/// inferred from a later episode of the same season that has already aired (episodes air in
/// order). `None` means unknown. AniList, for example, has no schedule entries for episodes
/// released together in a multi-episode premiere.
fn aired_states(episodes: &[EpisodeInfo], now: &str) -> Vec<Option<bool>> {
    let mut last_aired: HashMap<i32, i32> = HashMap::new();
    for e in episodes {
        if e.air_date_utc.as_deref().is_some_and(|a| a <= now) {
            let n = last_aired.entry(e.season_number).or_insert(i32::MIN);
            *n = (*n).max(e.episode_number);
        }
    }
    episodes
        .iter()
        .map(|e| match e.air_date_utc.as_deref() {
            Some(a) => Some(a <= now),
            None if last_aired
                .get(&e.season_number)
                .is_some_and(|n| *n > e.episode_number) =>
            {
                Some(true)
            }
            None => None,
        })
        .collect()
}

/// Initial monitoring for an episode (or, with `aired = None`, a season) of a new series.
/// Specials are never monitored automatically; "future" includes episodes with unknown dates.
fn episode_monitored(option: MonitorOption, season: i32, latest: i32, aired: Option<bool>) -> bool {
    if season == 0 {
        return false;
    }
    match option {
        MonitorOption::All => true,
        MonitorOption::Future => aired != Some(true),
        MonitorOption::FirstSeason => season == 1,
        MonitorOption::LatestSeason => season == latest,
        MonitorOption::None => false,
    }
}

fn movie_status(s: SeriesStatus) -> MovieStatus {
    match s {
        SeriesStatus::Ended => MovieStatus::Released,
        SeriesStatus::Upcoming => MovieStatus::Announced,
        SeriesStatus::Continuing => MovieStatus::InCinemas,
        SeriesStatus::Deleted => MovieStatus::Deleted,
        SeriesStatus::Unknown => MovieStatus::Unknown,
    }
}

fn ratings(info: &SeriesInfo) -> Option<serde_json::Value> {
    info.rating
        .map(|v| serde_json::json!({ "value": v, "votes": info.votes.unwrap_or(0) }))
}

/// Adds a title from a metadata catalogue.
pub async fn add_title(
    db: &Database,
    metadata: &Metadata,
    req: &AddTitleRequest,
) -> LibraryResult<LibraryRef> {
    let root = root_folders::normalize_path(&req.root_folder_path);
    let roots = root_folders::list(db.pool()).await?;
    if !roots.iter().any(|r| r.path == root) {
        return Err(LibraryError::invalid(
            "rootFolderPath",
            "Choose one of the configured root folders.",
        ));
    }
    kitsunarr_db::quality::get_profile(db.pool(), req.quality_profile_id)
        .await
        .map_err(|_| {
            LibraryError::invalid("qualityProfileId", "The quality profile does not exist.")
        })?;
    let known_tags = kitsunarr_db::tags::existing_ids(db.pool()).await?;
    if req.tags.iter().any(|t| !known_tags.contains(t)) {
        return Err(LibraryError::invalid(
            "tags",
            "One of the tags does not exist.",
        ));
    }
    let info = metadata.series(req.provider, &req.id).await?;
    if let Some(existing) = find_existing(db, req.provider, &req.id, &info.external_ids).await? {
        return Err(LibraryError::Exists(existing));
    }
    let naming_settings: NamingSettings =
        settings::get_or_default(db.pool(), settings::KEY_NAMING).await?;
    let now = kitsunarr_db::now();

    if info.kind == EntryKind::Movie {
        let folder = naming::folder_name(
            &naming_settings.movie_folder_format,
            &info.title,
            info.year,
            naming_settings.colon_replacement_format,
            &naming_settings.custom_colon_replacement_format,
        );
        let mut conn = db.pool().acquire().await?;
        let id = movies::insert(
            &mut conn,
            &movies::NewMovie {
                title: info.title.clone(),
                original_title: info.original_title.clone(),
                year: info.year,
                status: movie_status(info.status),
                overview: info.overview.clone(),
                studio: info.network.clone(),
                runtime: info.runtime,
                certification: None,
                original_language: info.original_language,
                genres: info.genres.clone(),
                images: info.images.clone(),
                ratings: ratings(&info),
                in_cinemas: info.first_aired.clone(),
                digital_release: None,
                physical_release: None,
                metadata_provider: info.provider,
                metadata_id: info.id.clone(),
                monitored: req.monitor != MonitorOption::None,
                minimum_availability: MinimumAvailability::Released,
                quality_profile_id: req.quality_profile_id,
                root_folder_path: root.clone(),
                path: naming::join(&root, &folder),
                added_at: now,
                last_refreshed_at: Some(kitsunarr_db::now()),
                tags: req.tags.clone(),
                external_ids: info.external_ids.clone(),
            },
        )
        .await?;
        tracing::info!(title = %info.title, provider = %info.provider, "movie added");
        return Ok(LibraryRef {
            media_type: "movie".into(),
            id,
        });
    }

    let series_type = req.series_type.unwrap_or(info.series_type);
    let kind = if series_type == SeriesType::Anime || info.kind == EntryKind::Anime {
        MediaKind::Anime
    } else {
        MediaKind::Series
    };
    let folder = file_names::series_folder(
        &file_names::Series::from_parts(&info.title, info.year, series_type, &info.external_ids),
        &naming_settings,
    );
    let latest = info
        .episodes
        .iter()
        .map(|e| e.season_number)
        .max()
        .unwrap_or(1);
    let mut tx = db.pool().begin().await?;
    let id = series::insert(
        &mut tx,
        &series::NewSeries {
            media_kind: kind,
            title: info.title.clone(),
            year: info.year,
            status: info.status,
            overview: info.overview.clone(),
            network: info.network.clone(),
            air_time: info.air_time.clone(),
            runtime: info.runtime,
            certification: None,
            original_language: info.original_language,
            genres: info.genres.clone(),
            images: info.images.clone(),
            ratings: ratings(&info),
            first_aired: info.first_aired.clone(),
            last_aired: info.last_aired.clone(),
            series_type,
            metadata_provider: info.provider,
            metadata_id: info.id.clone(),
            episode_ordering: None,
            monitored: true,
            monitor_new_items: if req.monitor == MonitorOption::None {
                MonitorNewItems::None
            } else {
                MonitorNewItems::All
            },
            season_folder: req.season_folder,
            use_scene_numbering: false,
            quality_profile_id: req.quality_profile_id,
            root_folder_path: root.clone(),
            path: naming::join(&root, &folder),
            added_at: now.clone(),
            last_refreshed_at: Some(now.clone()),
            tags: req.tags.clone(),
            external_ids: info.external_ids.clone(),
            alternate_titles: info
                .alternate_titles
                .iter()
                .map(|t| (t.clone(), None, Some("provider".to_string())))
                .collect(),
        },
    )
    .await?;
    let mut season_numbers: Vec<i32> = info
        .seasons
        .iter()
        .map(|s| s.season_number)
        .chain(info.episodes.iter().map(|e| e.season_number))
        .collect();
    season_numbers.sort_unstable();
    season_numbers.dedup();
    for n in season_numbers {
        let title = info
            .seasons
            .iter()
            .find(|s| s.season_number == n)
            .and_then(|s| s.title.clone());
        let monitored = episode_monitored(req.monitor, n, latest, None);
        series::upsert_season(
            &mut tx,
            id,
            &series::NewSeason {
                season_number: n,
                monitored,
                title,
                images: vec![],
            },
        )
        .await?;
    }
    let aired = aired_states(&info.episodes, &now);
    for (e, aired) in info.episodes.iter().zip(aired) {
        series::upsert_episode(
            &mut tx,
            &series::NewEpisode {
                series_id: id,
                season_number: e.season_number,
                episode_number: e.episode_number,
                absolute_episode_number: e.absolute_episode_number,
                scene_season_number: None,
                scene_episode_number: None,
                scene_absolute_episode_number: None,
                unverified_scene_numbering: false,
                title: e.title.clone(),
                overview: e.overview.clone(),
                air_date: e.air_date.clone(),
                air_date_utc: e.air_date_utc.clone(),
                runtime: e.runtime,
                monitored: episode_monitored(req.monitor, e.season_number, latest, aired),
                episode_file_id: None,
                provider_episode_id: e.provider_episode_id.clone(),
                last_search_at: None,
            },
        )
        .await?;
    }
    tx.commit().await?;
    tracing::info!(title = %info.title, provider = %info.provider, episodes = info.episodes.len(), "series added");
    Ok(LibraryRef {
        media_type: "series".into(),
        id,
    })
}

/// Refreshes a series from its owning catalogue.
pub async fn refresh_series(
    db: &Database,
    metadata: &Metadata,
    id: i64,
) -> LibraryResult<RefreshResult> {
    let (provider, metadata_id, monitored, monitor_new, ordering): (
        String,
        String,
        bool,
        String,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT metadata_provider, metadata_id, monitored, monitor_new_items, episode_ordering FROM series WHERE id = ?",
    )
            .bind(id)
            .fetch_optional(db.pool())
            .await?
            .ok_or(LibraryError::Db(DbError::NotFound))?;
    let provider: MetadataProvider = provider
        .parse()
        .map_err(|_| LibraryError::invalid("metadataProvider", "Unknown metadata provider."))?;
    if !Metadata::supports(provider) {
        return Err(LibraryError::Metadata(MetadataError::Unsupported(provider)));
    }
    let info = metadata
        .series_fresh(provider, &metadata_id, ordering.as_deref())
        .await?;
    let monitor_new = monitored && monitor_new == "all";
    let mut tx = db.pool().begin().await?;
    // Like Sonarr, a newly discovered episode is monitored only when new items are monitored
    // and its season is monitored; seasons created now get the default (specials excluded).
    let season_flags: HashMap<i64, bool> =
        sqlx::query_as("SELECT season_number, monitored FROM seasons WHERE series_id = ?")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    let season_monitored = |n: i32| {
        season_flags
            .get(&i64::from(n))
            .copied()
            .unwrap_or(monitor_new && n > 0)
    };
    series::update_metadata(
        &mut tx,
        id,
        &series::SeriesMetadata {
            title: info.title.clone(),
            year: info.year,
            status: info.status,
            overview: info.overview.clone(),
            network: info.network.clone(),
            air_time: info.air_time.clone(),
            runtime: info.runtime,
            original_language: info.original_language,
            genres: info.genres.clone(),
            images: info.images.clone(),
            ratings: ratings(&info),
            first_aired: info.first_aired.clone(),
            last_aired: info.last_aired.clone(),
            external_ids: info.external_ids.clone(),
            alternate_titles: info.alternate_titles.clone(),
        },
    )
    .await?;
    for s in &info.seasons {
        series::ensure_season(
            &mut tx,
            id,
            s.season_number,
            monitor_new && s.season_number > 0,
            s.title.as_deref(),
        )
        .await?;
    }
    let (mut added, mut updated) = (0u64, 0u64);
    for e in &info.episodes {
        series::ensure_season(
            &mut tx,
            id,
            e.season_number,
            monitor_new && e.season_number > 0,
            None,
        )
        .await?;
        let inserted = series::merge_episode(
            &mut tx,
            &series::NewEpisode {
                series_id: id,
                season_number: e.season_number,
                episode_number: e.episode_number,
                absolute_episode_number: e.absolute_episode_number,
                scene_season_number: None,
                scene_episode_number: None,
                scene_absolute_episode_number: None,
                unverified_scene_numbering: false,
                title: e.title.clone(),
                overview: e.overview.clone(),
                air_date: e.air_date.clone(),
                air_date_utc: e.air_date_utc.clone(),
                runtime: e.runtime,
                monitored: monitor_new && season_monitored(e.season_number),
                episode_file_id: None,
                provider_episode_id: e.provider_episode_id.clone(),
                last_search_at: None,
            },
        )
        .await?;
        if inserted { added += 1 } else { updated += 1 }
    }
    tx.commit().await?;
    Ok(RefreshResult {
        added_episodes: added,
        updated_episodes: updated,
    })
}

/// Refreshes a movie from its owning catalogue.
pub async fn refresh_movie(db: &Database, metadata: &Metadata, id: i64) -> LibraryResult<()> {
    let (provider, metadata_id): (String, String) =
        sqlx::query_as("SELECT metadata_provider, metadata_id FROM movies WHERE id = ?")
            .bind(id)
            .fetch_optional(db.pool())
            .await?
            .ok_or(LibraryError::Db(DbError::NotFound))?;
    let provider: MetadataProvider = provider
        .parse()
        .map_err(|_| LibraryError::invalid("metadataProvider", "Unknown metadata provider."))?;
    if !Metadata::supports(provider) {
        return Err(LibraryError::Metadata(MetadataError::Unsupported(provider)));
    }
    let info = metadata.series_fresh(provider, &metadata_id, None).await?;
    let mut conn = db.pool().acquire().await?;
    movies::update_metadata(
        &mut conn,
        id,
        &info.title,
        info.original_title.as_deref(),
        info.year,
        movie_status(info.status),
        info.overview.as_deref(),
        info.network.as_deref(),
        info.runtime,
        &info.genres,
        &info.images,
        info.first_aired.as_deref(),
        &info.external_ids,
    )
    .await?;
    Ok(())
}

/// Ids of series and movies that are due for a metadata refresh, least recently refreshed first.
///
/// Mirrors Sonarr's policy: titles are refreshed at most every 6 hours; ended (or removed)
/// series and released movies only monthly unless an episode aired in the last 30 days.
/// Only titles owned by a catalogue that can be queried now are returned (TMDB and OMDb need
/// their API keys).
pub async fn due_for_refresh(
    db: &Database,
    metadata: &Metadata,
    now: DateTime<Utc>,
) -> LibraryResult<(Vec<i64>, Vec<i64>)> {
    let providers: Vec<&str> = MetadataProvider::ALL
        .iter()
        .filter(|p| metadata.available(**p))
        .map(|p| p.as_str())
        .collect();
    let placeholders = providers.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let fmt = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Secs, true);
    let recent = fmt(now - TimeDelta::hours(6));
    let month = fmt(now - TimeDelta::days(30));
    let month_date = (now - TimeDelta::days(30)).format("%Y-%m-%d").to_string();
    let mut series_q = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM series WHERE metadata_provider IN ({placeholders}) AND ( \
           last_refreshed_at IS NULL OR last_refreshed_at < ? \
           OR (last_refreshed_at < ? AND (status NOT IN ('ended', 'deleted') OR COALESCE(last_aired, '') >= ?)) \
         ) ORDER BY COALESCE(last_refreshed_at, '') ASC, id ASC"
    )));
    let mut movie_q = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM movies WHERE metadata_provider IN ({placeholders}) AND ( \
           last_refreshed_at IS NULL OR last_refreshed_at < ? \
           OR (last_refreshed_at < ? AND status NOT IN ('released', 'deleted')) \
         ) ORDER BY COALESCE(last_refreshed_at, '') ASC, id ASC"
    )));
    for p in &providers {
        series_q = series_q.bind(*p);
        movie_q = movie_q.bind(*p);
    }
    let series_ids = series_q
        .bind(&month)
        .bind(&recent)
        .bind(&month_date)
        .fetch_all(db.pool())
        .await?;
    let movie_ids = movie_q
        .bind(&month)
        .bind(&recent)
        .fetch_all(db.pool())
        .await?;
    Ok((series_ids, movie_ids))
}

/// The folder a title would get under a root folder (`GetSeriesFolder` for series, the movie
/// folder format for movies), as the add dialog previews it.
pub async fn folder_preview(
    db: &Database,
    title: &str,
    year: Option<i32>,
    movie: bool,
) -> Result<String, DbError> {
    let naming_settings: NamingSettings =
        settings::get_or_default(db.pool(), settings::KEY_NAMING).await?;
    Ok(if movie {
        naming::folder_name(
            &naming_settings.movie_folder_format,
            title,
            year,
            naming_settings.colon_replacement_format,
            &naming_settings.custom_colon_replacement_format,
        )
    } else {
        file_names::series_folder(
            &file_names::Series::from_parts(title, year, SeriesType::Standard, &[]),
            &naming_settings,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitoring_options() {
        use MonitorOption as M;
        assert!(
            !episode_monitored(M::All, 0, 3, None),
            "specials are never monitored by default"
        );
        assert!(episode_monitored(M::All, 2, 3, Some(true)));
        assert!(episode_monitored(M::Future, 1, 3, Some(false)));
        assert!(!episode_monitored(M::Future, 1, 3, Some(true)));
        assert!(
            episode_monitored(M::Future, 1, 3, None),
            "unknown air date counts as future"
        );
        assert!(episode_monitored(M::FirstSeason, 1, 3, None));
        assert!(!episode_monitored(M::FirstSeason, 2, 3, None));
        assert!(episode_monitored(M::LatestSeason, 3, 3, None));
        assert!(!episode_monitored(M::None, 1, 3, None));
    }

    fn ep(season: i32, number: i32, air: Option<&str>) -> EpisodeInfo {
        EpisodeInfo {
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            title: None,
            overview: None,
            air_date: None,
            air_date_utc: air.map(str::to_string),
            runtime: None,
            provider_episode_id: None,
        }
    }

    #[test]
    fn airing_is_inferred_from_later_episodes() {
        let now = "2026-01-01T00:00:00Z";
        let episodes = [
            ep(1, 1, None),
            ep(1, 2, None),
            ep(1, 3, Some("2025-06-01T00:00:00Z")),
            ep(1, 4, Some("2026-06-01T00:00:00Z")),
            ep(1, 5, None),
            ep(2, 1, None),
        ];
        assert_eq!(
            aired_states(&episodes, now),
            vec![Some(true), Some(true), Some(true), Some(false), None, None],
            "a premiere without schedule entries aired before episode 3; nothing is known about S01E05 or season 2"
        );
    }
}
