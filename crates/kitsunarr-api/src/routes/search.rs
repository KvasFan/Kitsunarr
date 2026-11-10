//! Indexer tests and interactive release search.

use axum::Json;
use axum::extract::{Query, State};
use kitsunarr_db::library::series;
use kitsunarr_db::{providers, quality, rules};
use kitsunarr_decision::{self as decision, Decision, Engine, SearchKind};
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::library::{EpisodeItem, SeriesDetail};
use kitsunarr_domain::api::search::{
    IndexerFailure, IndexerTestRequest, IndexerTestResult, ReleaseItem, ReleaseQuery,
    ReleaseSearchResult,
};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_domain::quality::{QualityModel, Revision};
use kitsunarr_domain::{DownloadProtocol, ExternalIdKind, Language, SeriesType};
use kitsunarr_indexers::feed::flags;
use kitsunarr_indexers::request::{clean_scene_title, search_mode};
use kitsunarr_indexers::{
    Criteria, Dialect, Indexer, IndexerError, IndexerRelease, Search, SeriesIds, Settings,
};
use kitsunarr_parser::ReleaseType;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

fn dialect(implementation: &str) -> Option<Dialect> {
    match implementation {
        "Newznab" => Some(Dialect::Newznab),
        "Torznab" => Some(Dialect::Torznab),
        _ => None,
    }
}

/// A user-facing explanation of an indexer error.
fn describe(e: &IndexerError) -> String {
    match e {
        IndexerError::ApiKey(m) => format!("The indexer rejected the API key: {m}"),
        IndexerError::RequestLimit { .. } => {
            "The indexer's request limit was reached; try again later".into()
        }
        IndexerError::Indexer(m) => m.clone(),
        IndexerError::Connection(m) => format!("Could not connect to the indexer: {m}"),
        IndexerError::Http { status } => format!("The indexer answered HTTP {status}"),
        IndexerError::InvalidXml(m) | IndexerError::Unexpected(m) => {
            format!("Unexpected response from the indexer: {m}")
        }
    }
}

/// Test an indexer's settings: capabilities, plus the newest page of results.
#[utoipa::path(post, path = "/providers/indexers/test", tag = "search", request_body = IndexerTestRequest,
    responses(
        (status = 200, body = IndexerTestResult, description = "Test outcome (ok or not)"),
        (status = 422, body = ApiErrorBody)
    ))]
pub async fn test_indexer(
    State(state): State<AppState>,
    Json(mut req): Json<IndexerTestRequest>,
) -> ApiResult<Json<IndexerTestResult>> {
    let dialect = dialect(&req.implementation).ok_or_else(|| {
        ApiError::field(
            "implementation",
            format!(
                "Testing {} indexers is not supported yet.",
                req.implementation
            ),
        )
    })?;
    let mut name = "Indexer".to_string();
    if let Some(id) = req.id {
        let stored = providers::get(state.db.pool(), ProviderKind::Indexer, id).await?;
        crate::routes::settings::merge_secrets(&mut req.settings, &stored.settings);
        name = stored.name;
    }
    let settings = Settings::from_map(&req.settings).map_err(|m| ApiError::field("baseUrl", m))?;
    let indexer = Indexer {
        id: req.id.unwrap_or(0),
        name,
        dialect,
        settings,
    };
    Ok(Json(match state.indexers.test(&indexer).await {
        Ok(report) => IndexerTestResult {
            ok: true,
            message: format!(
                "Connected: {} releases in the newest page.",
                report.releases_found
            ),
            releases_found: report.releases_found,
            warnings: report.warnings,
        },
        Err(e) => IndexerTestResult {
            ok: false,
            message: describe(&e),
            releases_found: 0,
            warnings: Vec::new(),
        },
    }))
}

fn flag_names(bits: u32) -> Vec<String> {
    [
        (flags::FREELEECH, "Freeleech"),
        (flags::HALFLEECH, "Halfleech"),
        (flags::DOUBLE_UPLOAD, "DoubleUpload"),
        (flags::INTERNAL, "Internal"),
        (flags::SCENE, "Scene"),
        (flags::FREELEECH75, "Freeleech75"),
        (flags::FREELEECH25, "Freeleech25"),
        (flags::NUKED, "Nuked"),
        (flags::SUBTITLES, "Subtitles"),
    ]
    .iter()
    .filter(|(f, _)| bits & f != 0)
    .map(|(_, n)| n.to_string())
    .collect()
}

fn to_item(r: IndexerRelease, d: &Decision, series_id: i64) -> ReleaseItem {
    let parsed = d.parsed.as_ref();
    ReleaseItem {
        guid: r.release.guid,
        title: r.release.title,
        indexer_id: r.indexer_id,
        indexer: r.indexer,
        protocol: r.release.protocol,
        size: r.release.size,
        publish_date: r
            .release
            .publish_date
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        info_url: r.release.info_url,
        seeders: r.release.seeders,
        peers: r.release.peers,
        indexer_flags: flag_names(r.release.indexer_flags),
        quality: d.quality.quality.name.to_string(),
        quality_weight: d.quality.quality.weight,
        languages: d.languages.iter().map(|l| l.name.to_string()).collect(),
        release_group: parsed.and_then(|p| p.release_group.clone()),
        parsed: parsed.is_some(),
        series_title: parsed.map(|p| p.series_title.clone()),
        season_numbers: parsed.map(|p| p.season_numbers.clone()).unwrap_or_default(),
        episode_numbers: parsed
            .map(|p| p.episode_numbers.clone())
            .unwrap_or_default(),
        absolute_episode_numbers: parsed
            .map(|p| p.absolute_episode_numbers.clone())
            .unwrap_or_default(),
        full_season: parsed.is_some_and(|p| p.full_season),
        special: parsed.is_some_and(|p| p.special),
        matches_series: d.series_id == Some(series_id),
        approved: d.approved(),
        rejections: d.rejections.iter().map(|r| r.message.clone()).collect(),
        custom_formats: d.custom_formats.clone(),
        custom_format_score: d.custom_format_score,
        mapped_episodes: d
            .episodes
            .iter()
            .map(|e| format!("S{:02}E{:02}", e.season_number, e.episode_number))
            .collect(),
    }
}

/// `ReleaseSearchService.EpisodeSearch`: the searches for one episode, with a description.
fn episode_searches(
    series_type: SeriesType,
    scene_titles: &[String],
    e: &EpisodeItem,
) -> ApiResult<(Vec<(Search, SearchKind)>, String)> {
    let label = format!("S{:02}E{:02}", e.season_number, e.episode_number);
    let single = (
        Search::SingleEpisode {
            season: e.season_number,
            episode: e.episode_number,
        },
        SearchKind::SingleEpisode {
            season: e.season_number,
            episode: e.episode_number,
        },
    );
    // `SearchSpecial`: "<series> <episode title>" queries, then the season/episode search.
    let special = || {
        let mut queries: Vec<String> = Vec::new();
        if let Some(title) = e.title.as_deref().filter(|t| !t.trim().is_empty()) {
            let episode_title = clean_scene_title(title);
            // `CleanSceneTitles` is distinct case-sensitively, the queries case-insensitively.
            let mut clean: Vec<String> = Vec::new();
            for t in scene_titles.iter().map(|t| clean_scene_title(t)) {
                if !clean.contains(&t) {
                    clean.push(t);
                }
            }
            for c in clean {
                let query = format!("{c} {episode_title}");
                if !queries
                    .iter()
                    .any(|q| q.to_lowercase() == query.to_lowercase())
                {
                    queries.push(query);
                }
            }
        }
        (
            vec![
                (
                    Search::Special {
                        episode_query_titles: queries,
                    },
                    SearchKind::Special,
                ),
                single.clone(),
            ],
            format!("{label} (special)"),
        )
    };
    Ok(match series_type {
        SeriesType::Daily => {
            let date = e
                .air_date
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .ok_or_else(|| {
                    ApiError::field(
                        "episodeId",
                        "The episode has no air date, which daily searches need. Refresh the series and try again.",
                    )
                })?;
            (
                vec![(Search::DailyEpisode { date }, SearchKind::DailyEpisode)],
                format!("{label} ({date})"),
            )
        }
        SeriesType::Anime if e.season_number == 0 && e.absolute_episode_number.is_none() => {
            special()
        }
        SeriesType::Anime => {
            let absolute = e.absolute_episode_number.unwrap_or(0);
            (
                vec![(
                    Search::AnimeEpisode {
                        absolute,
                        season: e.season_number,
                        episode: e.episode_number,
                    },
                    SearchKind::AnimeEpisode {
                        season_search: false,
                    },
                )],
                format!("{label} (absolute {absolute})"),
            )
        }
        _ if e.season_number == 0 => special(),
        _ => (vec![single], label),
    })
}

/// Search every indexer enabled for interactive search for a season or an episode.
#[utoipa::path(get, path = "/releases", tag = "search", params(ReleaseQuery),
    responses(
        (status = 200, body = ReleaseSearchResult),
        (status = 404, body = ApiErrorBody),
        (status = 422, body = ApiErrorBody)
    ))]
pub async fn search_releases(
    State(state): State<AppState>,
    Query(q): Query<ReleaseQuery>,
) -> ApiResult<Json<ReleaseSearchResult>> {
    let detail = series::get_detail(&state.db, q.series_id).await?;
    let id_of = |kind: ExternalIdKind| {
        detail
            .external_ids
            .iter()
            .find(|e| e.kind == kind)
            .map(|e| e.value.clone())
    };
    let int_id = |kind| {
        id_of(kind)
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0)
    };
    let mut scene_titles = vec![detail.item.title.clone()];
    for t in &detail.alternate_titles {
        if !scene_titles.contains(t) {
            scene_titles.push(t.clone());
        }
    }
    let anime = detail.item.series_type == SeriesType::Anime;
    let criteria = Criteria {
        ids: SeriesIds {
            tvdb: int_id(ExternalIdKind::Tvdb),
            imdb: id_of(ExternalIdKind::Imdb),
            tv_rage: int_id(ExternalIdKind::TvRage),
            tv_maze: int_id(ExternalIdKind::TvMaze),
            tmdb: int_id(ExternalIdKind::Tmdb),
        },
        scene_titles,
        search_mode: search_mode::DEFAULT,
        anime,
    };

    let episodes = series::list_episodes(&state.db, q.series_id, None).await?;
    let (searches, description, requested): (Vec<(Search, SearchKind)>, String, Vec<i64>) =
        match (q.episode_id, q.season_number) {
            (Some(episode_id), _) => {
                let e = episodes
                    .iter()
                    .find(|e| e.id == episode_id)
                    .ok_or_else(|| ApiError::NotFound("The episode does not exist.".into()))?;
                let (searches, description) =
                    episode_searches(detail.item.series_type, &criteria.scene_titles, e)?;
                (searches, description, vec![episode_id])
            }
            (None, Some(season)) => {
                let search = if anime {
                    (
                        Search::AnimeSeason {
                            season,
                            season_scene_titles: Vec::new(),
                        },
                        SearchKind::AnimeSeason,
                    )
                } else {
                    (Search::Season { season }, SearchKind::Season { season })
                };
                let requested = episodes
                    .iter()
                    .filter(|e| e.season_number == season)
                    .map(|e| e.id)
                    .collect();
                (vec![search], format!("Season {season}"), requested)
            }
            (None, None) => {
                return Err(ApiError::field(
                    "seasonNumber",
                    "Choose a season or an episode to search for.",
                ));
            }
        };

    // Indexers that cannot be searched are reported, not skipped silently.
    let mut indexers = Vec::new();
    let mut extras = std::collections::HashMap::new();
    let mut failures = Vec::new();
    for p in providers::list(state.db.pool(), ProviderKind::Indexer).await? {
        let interactive = p
            .options
            .get("enableInteractiveSearch")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !interactive {
            continue;
        }
        let settings = match dialect(&p.implementation) {
            None => Err(format!(
                "Searching {} indexers is not supported yet.",
                p.implementation
            )),
            Some(dialect) => Settings::from_map(&p.settings)
                .map(|settings| (dialect, settings))
                .map_err(|m| format!("The indexer's settings are invalid: {m}")),
        };
        match settings {
            Ok((dialect, settings)) => {
                extras.insert(p.id, IndexerExtras::of(&p, dialect, &settings));
                indexers.push(Indexer {
                    id: p.id,
                    name: p.name,
                    dialect,
                    settings,
                });
            }
            Err(message) => failures.push(IndexerFailure {
                indexer_id: p.id,
                indexer: p.name,
                message,
            }),
        }
    }

    let client = &state.indexers;
    let outcomes = futures::future::join_all(indexers.iter().map(|indexer| {
        let searches = &searches;
        let criteria = &criteria;
        async move {
            let caps = client.capabilities(indexer).await?;
            let generator = client.generator(indexer, &caps);
            let mut found = Vec::new();
            for (k, (search, _)) in searches.iter().enumerate() {
                let chain = generator.search(criteria, search);
                found.extend(
                    client
                        .fetch(indexer, &chain, false)
                        .await?
                        .into_iter()
                        .map(|r| (k, r)),
                );
            }
            Ok::<_, IndexerError>(found)
        }
    }))
    .await;

    // Each search's releases, in arrival order.
    let mut found: Vec<Vec<IndexerRelease>> = vec![Vec::new(); searches.len()];
    for (indexer, outcome) in indexers.iter().zip(outcomes) {
        match outcome {
            Ok(list) => {
                for (k, r) in list {
                    found[k].push(r);
                }
            }
            Err(e) => {
                tracing::warn!(indexer = %indexer.name, error = %e, "interactive search failed");
                failures.push(IndexerFailure {
                    indexer_id: indexer.id,
                    indexer: indexer.name.clone(),
                    message: describe(&e),
                });
            }
        }
    }

    let inputs = DecisionInputs::load(&state, &detail, &episodes).await?;
    let engine = Engine {
        series: &inputs.series,
        library: &inputs.library,
        formats: &inputs.formats,
        settings: &inputs.settings,
    };
    // Each search's releases are decided with its own criteria, then merged
    // (`SearchSpecial` decides its two searches separately, then `DeDupeDecisions`).
    let mut all_meta: Vec<IndexerRelease> = Vec::new();
    let mut all: Vec<decision::Release> = Vec::new();
    let mut decisions = Vec::new();
    for ((_, kind), list) in searches.iter().zip(found) {
        let offset = all.len();
        let group: Vec<decision::Release> = list
            .iter()
            .map(|r| decision_release(r, extras.get(&r.indexer_id)))
            .collect();
        let criteria = decision::SearchCriteria {
            kind: *kind,
            episode_ids: requested.clone(),
        };
        decisions.extend(engine.evaluate(&group, &criteria).into_iter().map(|mut d| {
            d.release += offset;
            d
        }));
        all.extend(group);
        all_meta.extend(list);
    }
    let decisions = engine.prioritize(Engine::dedupe(decisions, &all), &all);
    let releases = decisions
        .iter()
        .map(|d| to_item(all_meta[d.release].clone(), d, detail.item.id))
        .collect();
    Ok(Json(ReleaseSearchResult {
        releases,
        failures,
        searched_indexers: indexers.len(),
        description,
        warnings: inputs.warnings,
    }))
}

/// Indexer settings the decisions read.
struct IndexerExtras {
    priority: i32,
    tags: Vec<i64>,
    minimum_seeders: Option<i64>,
    multi_languages: Vec<i32>,
}

impl IndexerExtras {
    fn of(p: &providers::ProviderRecord, dialect: Dialect, settings: &Settings) -> Self {
        Self {
            priority: p.priority,
            tags: p.tags.clone(),
            // Torznab's default is 1 (`IndexerDefaults.MINIMUM_SEEDERS`).
            minimum_seeders: (dialect == Dialect::Torznab)
                .then(|| i64::from(settings.minimum_seeders.unwrap_or(1))),
            multi_languages: p
                .settings
                .get("multiLanguages")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_i64().map(|i| i as i32))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

fn decision_release(r: &IndexerRelease, extras: Option<&IndexerExtras>) -> decision::Release {
    let rel = &r.release;
    decision::Release {
        guid: rel.guid.clone(),
        title: rel.title.clone(),
        size: rel.size,
        publish_date: rel.publish_date,
        protocol: rel.protocol,
        indexer_id: r.indexer_id,
        indexer_priority: extras.map_or(25, |e| e.priority),
        indexer_tags: extras.map(|e| e.tags.clone()).unwrap_or_default(),
        indexer_flags: rel.indexer_flags,
        languages: rel.languages.clone(),
        seeders: rel.seeders,
        peers: rel.peers,
        tvdb_id: rel.tvdb_id,
        tv_rage_id: rel.tv_rage_id,
        imdb_id: rel.imdb_id.clone(),
        minimum_seeders: extras.and_then(|e| e.minimum_seeders),
        multi_languages: extras
            .map(|e| e.multi_languages.clone())
            .unwrap_or_default(),
    }
}

/// The series, library, custom formats and settings a search's decisions need.
struct DecisionInputs {
    series: decision::Series,
    library: Vec<decision::LibrarySeries>,
    formats: Vec<decision::CompiledFormat>,
    settings: decision::Settings,
    warnings: Vec<String>,
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn release_type(id: i64) -> ReleaseType {
    match id {
        1 => ReleaseType::SingleEpisode,
        2 => ReleaseType::MultiEpisode,
        3 => ReleaseType::SeasonPack,
        4 => ReleaseType::MultiSeasonPack,
        _ => ReleaseType::Unknown,
    }
}

fn ids_of(external: &[(String, String)]) -> (i64, i64, Option<String>) {
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

impl DecisionInputs {
    async fn load(
        state: &AppState,
        detail: &SeriesDetail,
        episodes: &[EpisodeItem],
    ) -> ApiResult<Self> {
        let db = &state.db;
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
        let series = decision::Series {
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
        };
        let library = series::title_records(db)
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
            .collect();
        let (formats, warnings) =
            decision::compile_formats(&quality::list_custom_formats(db.pool()).await?);
        let settings = decision::Settings {
            quality_definitions: quality::list_definitions(db.pool()).await?,
            propers: decision::ProperDownloadType::default(),
            release_profiles: rules::list_release_profiles(db.pool())
                .await?
                .into_iter()
                .map(|r| decision::ReleaseProfile {
                    enabled: r.enabled,
                    required: r.required,
                    ignored: r.ignored,
                    indexer_ids: r.indexer_ids,
                    tags: r.tags,
                })
                .collect(),
            delay_profiles: rules::list_delay_profiles(db.pool())
                .await?
                .into_iter()
                .map(|d| decision::DelayProfile {
                    enable_usenet: d.enable_usenet,
                    enable_torrent: d.enable_torrent,
                    preferred_protocol: if d.preferred_protocol == "torrent" {
                        DownloadProtocol::Torrent
                    } else {
                        DownloadProtocol::Usenet
                    },
                    order: d.sort_order,
                    tags: d.tags,
                })
                .collect(),
            minimum_age_minutes: 0,
            retention_days: 0,
            maximum_size_mb: 0,
            now: chrono::Utc::now(),
        };
        Ok(Self {
            series,
            library,
            formats,
            settings,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(season: i32, number: i32, absolute: Option<i32>, title: &str) -> EpisodeItem {
        serde_json::from_value(serde_json::json!({
            "id": 1, "seriesId": 1, "seasonNumber": season, "episodeNumber": number,
            "absoluteEpisodeNumber": absolute, "sceneSeasonNumber": null,
            "sceneEpisodeNumber": null, "title": title, "airDate": null, "airDateUtc": null,
            "runtime": null, "monitored": true, "file": null
        }))
        .unwrap()
    }

    fn searches(kind: SeriesType, titles: &[&str], e: &EpisodeItem) -> Vec<Search> {
        let titles: Vec<String> = titles.iter().map(|t| t.to_string()).collect();
        episode_searches(kind, &titles, e)
            .unwrap()
            .0
            .into_iter()
            .map(|(s, _)| s)
            .collect()
    }

    #[test]
    fn specials_search_by_episode_title_then_by_number() {
        let e = episode(0, 1, None, "Minisode: Good Cop / Bad Cop");
        assert_eq!(
            searches(SeriesType::Standard, &["Breaking Bad"], &e),
            vec![
                Search::Special {
                    episode_query_titles: vec!["Breaking+Bad Minisode+Good+Cop+Bad+Cop".into()]
                },
                Search::SingleEpisode {
                    season: 0,
                    episode: 1
                },
            ]
        );
        // Clean titles differing only in case give one query; an untitled special still
        // searches by number.
        let office = searches(SeriesType::Standard, &["The Office", "the office"], &e);
        assert!(
            matches!(&office[0], Search::Special { episode_query_titles } if episode_query_titles.len() == 1)
        );
        let untitled = searches(
            SeriesType::Standard,
            &["Breaking Bad"],
            &episode(0, 2, None, " "),
        );
        assert_eq!(
            untitled[0],
            Search::Special {
                episode_query_titles: vec![]
            }
        );
    }

    #[test]
    fn anime_and_daily_episodes_follow_upstream() {
        // Anime specials without an absolute number use the special search…
        let special = searches(
            SeriesType::Anime,
            &["Frieren"],
            &episode(0, 1, None, "Recap"),
        );
        assert!(matches!(special[0], Search::Special { .. }));
        // …other anime episodes the absolute number (0 when unknown, as upstream).
        assert_eq!(
            searches(SeriesType::Anime, &["Frieren"], &episode(1, 3, None, "x")),
            vec![Search::AnimeEpisode {
                absolute: 0,
                season: 1,
                episode: 3
            }]
        );
        assert_eq!(
            searches(
                SeriesType::Anime,
                &["Frieren"],
                &episode(0, 1, Some(29), "x")
            ),
            vec![Search::AnimeEpisode {
                absolute: 29,
                season: 0,
                episode: 1
            }]
        );
        // Daily episodes need an air date.
        let e = episode(2024, 1, None, "x");
        assert!(episode_searches(SeriesType::Daily, &["Show".into()], &e).is_err());
        let mut dated = e.clone();
        dated.air_date = Some("2024-03-05".into());
        assert_eq!(
            searches(SeriesType::Daily, &["Show"], &dated),
            vec![Search::DailyEpisode {
                date: chrono::NaiveDate::from_ymd_opt(2024, 3, 5).unwrap()
            }]
        );
    }
}
