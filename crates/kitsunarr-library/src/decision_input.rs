//! The library as the decision engine sees it (`kitsunarr_decision::Series` and the other
//! titles), shared by release searches and download imports.

use kitsunarr_db::library::series;
use kitsunarr_db::{Database, DbResult, quality};
use kitsunarr_decision as decision;
use kitsunarr_domain::Language;
use kitsunarr_domain::api::library::{EpisodeItem, SeriesDetail};
use kitsunarr_domain::quality::{QualityModel, Revision};
use kitsunarr_parser::ReleaseType;

/// The file name of a path (either separator).
pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

pub fn release_type(id: i64) -> ReleaseType {
    match id {
        1 => ReleaseType::SingleEpisode,
        2 => ReleaseType::MultiEpisode,
        3 => ReleaseType::SeasonPack,
        4 => ReleaseType::MultiSeasonPack,
        _ => ReleaseType::Unknown,
    }
}

/// Sonarr's `ReleaseType` name, as grab history stores it (`releaseType`).
pub fn release_type_name(t: ReleaseType) -> &'static str {
    match t {
        ReleaseType::Unknown => "Unknown",
        ReleaseType::SingleEpisode => "SingleEpisode",
        ReleaseType::MultiEpisode => "MultiEpisode",
        ReleaseType::SeasonPack => "SeasonPack",
        ReleaseType::MultiSeasonPack => "MultiSeasonPack",
    }
}

fn release_type_named(name: &str) -> ReleaseType {
    match name {
        n if n.eq_ignore_ascii_case("SingleEpisode") => ReleaseType::SingleEpisode,
        n if n.eq_ignore_ascii_case("MultiEpisode") => ReleaseType::MultiEpisode,
        n if n.eq_ignore_ascii_case("SeasonPack") => ReleaseType::SeasonPack,
        n if n.eq_ignore_ascii_case("MultiSeasonPack") => ReleaseType::MultiSeasonPack,
        _ => ReleaseType::Unknown,
    }
}

/// A history date: RFC 3339, or Sonarr's `yyyy-MM-dd HH:mm:ss[.fff]Z` (migrated history).
pub fn history_date(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(d.with_timezone(&chrono::Utc));
    }
    let t = text.trim().trim_end_matches('Z');
    chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S%.f"))
        .ok()
        .map(|d| d.and_utc())
}

/// A number stored in a history entry's data (Sonarr stores numbers as strings).
pub fn data_number(data: &serde_json::Value, key: &str) -> Option<i64> {
    data.get(key).and_then(|v| match v {
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        other => other.as_i64(),
    })
}

/// The series' blocklisted releases, for `BlocklistSpecification`.
pub async fn blocklist(db: &Database, series_id: i64) -> DbResult<Vec<decision::Blocklisted>> {
    Ok(kitsunarr_db::blocklist::for_series(db, series_id)
        .await?
        .into_iter()
        .map(|b| decision::Blocklisted {
            series_id: b.series_id.unwrap_or(0),
            title: b.source_title,
            protocol: b
                .protocol
                .parse()
                .unwrap_or(kitsunarr_domain::DownloadProtocol::Unknown),
            indexer: b.indexer,
            published_date: b.published_date.as_deref().and_then(history_date),
            torrent_info_hash: b.torrent_info_hash,
        })
        .collect())
}

/// A film's blocklisted releases, keyed by the film's id (the engine's series id for a film).
pub async fn movie_blocklist(db: &Database, movie_id: i64) -> DbResult<Vec<decision::Blocklisted>> {
    Ok(kitsunarr_db::blocklist::for_movie(db, movie_id)
        .await?
        .into_iter()
        .map(|b| decision::Blocklisted {
            series_id: movie_id,
            title: b.source_title,
            protocol: b
                .protocol
                .parse()
                .unwrap_or(kitsunarr_domain::DownloadProtocol::Unknown),
            indexer: b.indexer,
            published_date: b.published_date.as_deref().and_then(history_date),
            torrent_info_hash: b.torrent_info_hash,
        })
        .collect())
}

/// The series' episodes whose most recent history event is a grab, for the history rule.
pub async fn last_grabs(db: &Database, series_id: i64) -> DbResult<Vec<decision::HistoryGrab>> {
    Ok(kitsunarr_db::history::last_grabs_for_series(db, series_id)
        .await?
        .into_iter()
        .filter_map(|g| {
            let data: serde_json::Value = serde_json::from_str(&g.data).unwrap_or_default();
            let languages: Vec<i32> = g
                .languages
                .as_deref()
                .and_then(|l| serde_json::from_str(l).ok())
                .unwrap_or_default();
            Some(decision::HistoryGrab {
                episode_id: g.episode_id,
                date: history_date(&g.date)?,
                title: g.source_title,
                quality: QualityModel {
                    quality_id: g.quality_id.unwrap_or(0) as i32,
                    revision: g
                        .revision
                        .as_deref()
                        .and_then(|r| serde_json::from_str(r).ok())
                        .unwrap_or_default(),
                },
                size: data_number(&data, "size").unwrap_or(0),
                languages: languages.into_iter().filter_map(Language::by_id).collect(),
                indexer_flags: data_number(&data, "indexerFlags").unwrap_or(0) as u32,
                release_type: release_type_named(
                    data.get("releaseType")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default(),
                ),
            })
        })
        .collect())
}

pub fn ids_of(external: &[(String, String)]) -> (i64, i64, Option<String>) {
    let get = |kind: &str| {
        external
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, v)| v.trim().to_string())
    };
    let int = |kind: &str| get(kind).and_then(|v| v.parse().ok()).unwrap_or(0);
    (
        int("tvdb"),
        int("tvRage"),
        get("imdb").filter(|v| !v.is_empty()),
    )
}

/// A series with its episodes and files, for decisions and imports.
pub async fn series_model(
    db: &Database,
    detail: &SeriesDetail,
    episodes: &[EpisodeItem],
) -> DbResult<decision::Series> {
    let files: std::collections::HashMap<i64, series::FileRecord> =
        series::file_records(db, detail.item.id)
            .await?
            .into_iter()
            .map(|f| (f.id, f))
            .collect();
    let original_language = series::original_language(db, detail.item.id)
        .await?
        .and_then(|id| Language::by_id(id as i32));
    let profile = quality::get_profile(db.pool(), detail.item.quality_profile_id).await?;
    let external: Vec<(String, String)> = detail
        .external_ids
        .iter()
        .map(|e| (e.kind.to_string(), e.value.clone()))
        .collect();
    let (tvdb_id, tv_rage_id, imdb_id) = ids_of(&external);
    Ok(decision::Series {
        monitored: detail.item.monitored,
        id: detail.item.id,
        title: detail.item.title.clone(),
        year: detail.item.year.unwrap_or(0),
        series_type: detail.item.series_type,
        tvdb_id,
        tv_rage_id,
        imdb_id,
        alternate_titles: detail.alternate_titles.clone(),
        runtime: detail.runtime.unwrap_or(0),
        original_language,
        use_scene_numbering: detail.use_scene_numbering,
        tags: detail.item.tags.clone(),
        profile,
        episodes: episodes
            .iter()
            .map(|e| decision::Episode {
                id: e.id,
                season_number: e.season_number,
                episode_number: e.episode_number,
                absolute_episode_number: e.absolute_episode_number,
                scene_season_number: e.scene_season_number,
                scene_episode_number: e.scene_episode_number,
                title: e.title.clone(),
                air_date: e.air_date.clone(),
                air_date_utc: e
                    .air_date_utc
                    .as_deref()
                    .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                    .map(|d| d.with_timezone(&chrono::Utc)),
                runtime: e.runtime.unwrap_or(0),
                monitored: e.monitored,
                file: e.file.as_ref().and_then(|f| files.get(&f.id)).map(|f| {
                    decision::EpisodeFile {
                        id: f.id,
                        quality: QualityModel {
                            quality_id: f.quality_id as i32,
                            revision: Revision {
                                version: f.revision_version as i32,
                                real: f.revision_real as i32,
                                is_repack: f.is_repack,
                            },
                        },
                        // `ParseCustomFormat(EpisodeFile)`: scene name, else the original
                        // file name, else the file name.
                        release_title: f
                            .scene_name
                            .clone()
                            .filter(|n| !n.trim().is_empty())
                            .or_else(|| {
                                f.original_path
                                    .as_deref()
                                    .filter(|p| !p.trim().is_empty())
                                    .map(file_name)
                            })
                            .unwrap_or_else(|| file_name(&f.relative_path)),
                        file_name: file_name(&f.relative_path),
                        release_group: f.release_group.clone(),
                        languages: f
                            .language_ids()
                            .into_iter()
                            .filter_map(Language::by_id)
                            .collect(),
                        size: f.size,
                        indexer_flags: f.indexer_flags as u32,
                        release_type: release_type(f.release_type),
                        date_added: history_date(&f.added_at),
                    }
                }),
            })
            .collect(),
    })
}

/// Every library title, to recognise releases of other series.
pub async fn library_titles(db: &Database) -> DbResult<Vec<decision::LibrarySeries>> {
    Ok(series::title_records(db)
        .await?
        .into_iter()
        .map(|t| {
            let (tvdb_id, tv_rage_id, imdb_id) = ids_of(&t.external_ids);
            decision::LibrarySeries {
                id: t.id,
                title: t.title,
                year: t.year.unwrap_or(0) as i32,
                alternate_titles: t.alternate_titles,
                tvdb_id,
                tv_rage_id,
                imdb_id,
            }
        })
        .collect())
}

/// A library film as the decision engine's one-episode series (Tanukarr models a film as
/// S01E01): the episode carries the film's id, runtime, monitoring and file.
pub async fn movie_model(
    db: &Database,
    detail: &kitsunarr_domain::api::library::MovieDetail,
) -> DbResult<decision::Series> {
    let profile = quality::get_profile(db.pool(), detail.item.quality_profile_id).await?;
    let external: Vec<(String, String)> = detail
        .external_ids
        .iter()
        .map(|e| (e.kind.to_string(), e.value.clone()))
        .collect();
    let (_, _, imdb_id) = ids_of(&external);
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        relative_path: String,
        quality_id: i64,
        revision_version: i64,
        revision_real: i64,
        is_repack: bool,
        languages: String,
        release_group: Option<String>,
        scene_name: Option<String>,
        original_path: Option<String>,
        size: i64,
        added_at: String,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, relative_path, quality_id, revision_version, revision_real, is_repack, languages, \
         release_group, scene_name, original_path, size, added_at FROM movie_files WHERE movie_id = ?",
    )
    .bind(detail.item.id)
    .fetch_optional(db.pool())
    .await?;
    let file = row.map(|f| decision::EpisodeFile {
        id: f.id,
        quality: QualityModel {
            quality_id: f.quality_id as i32,
            revision: Revision {
                version: f.revision_version as i32,
                real: f.revision_real as i32,
                is_repack: f.is_repack,
            },
        },
        release_title: f
            .scene_name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                f.original_path
                    .as_deref()
                    .filter(|p| !p.trim().is_empty())
                    .map(file_name)
            })
            .unwrap_or_else(|| file_name(&f.relative_path)),
        file_name: file_name(&f.relative_path),
        release_group: f.release_group,
        languages: serde_json::from_str::<Vec<i32>>(&f.languages)
            .unwrap_or_default()
            .into_iter()
            .filter_map(Language::by_id)
            .collect(),
        size: f.size,
        indexer_flags: 0,
        release_type: kitsunarr_parser::ReleaseType::SingleEpisode,
        date_added: chrono::DateTime::parse_from_rfc3339(&f.added_at)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc)),
    });
    let runtime = detail.runtime.unwrap_or(0);
    let date = detail
        .digital_release
        .clone()
        .or_else(|| detail.physical_release.clone())
        .or_else(|| detail.in_cinemas.clone());
    Ok(decision::Series {
        monitored: detail.item.monitored,
        id: detail.item.id,
        title: detail.item.title.clone(),
        year: detail.item.year.unwrap_or(0),
        series_type: kitsunarr_domain::SeriesType::Standard,
        tvdb_id: 0,
        tv_rage_id: 0,
        imdb_id,
        alternate_titles: Vec::new(),
        runtime,
        original_language: None,
        use_scene_numbering: false,
        tags: detail.item.tags.clone(),
        profile,
        episodes: vec![decision::Episode {
            id: detail.item.id,
            season_number: 1,
            episode_number: 1,
            absolute_episode_number: None,
            scene_season_number: None,
            scene_episode_number: None,
            title: Some(detail.item.title.clone()),
            air_date: date.as_deref().map(|d| d.chars().take(10).collect()),
            air_date_utc: date
                .as_deref()
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .map(|d| d.with_timezone(&chrono::Utc)),
            runtime,
            monitored: detail.item.monitored,
            file,
        }],
    })
}
