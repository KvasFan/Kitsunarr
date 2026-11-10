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
