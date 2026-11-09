//! Indexer tests and interactive release search.

use axum::Json;
use axum::extract::{Query, State};
use kitsunarr_db::library::series;
use kitsunarr_db::providers;
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::search::{
    IndexerFailure, IndexerTestRequest, IndexerTestResult, ReleaseItem, ReleaseQuery,
    ReleaseSearchResult,
};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_domain::{ExternalIdKind, SeriesType};
use kitsunarr_indexers::feed::flags;
use kitsunarr_indexers::request::search_mode;
use kitsunarr_indexers::{
    Criteria, Dialect, Indexer, IndexerError, IndexerRelease, Search, SeriesIds, Settings,
};

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

fn to_item(r: IndexerRelease, clean_titles: &[String]) -> ReleaseItem {
    let parsed = kitsunarr_parser::parse_title(&r.release.title);
    let quality = parsed
        .as_ref()
        .map(|p| p.quality)
        .unwrap_or_else(|| kitsunarr_parser::parse_quality(&r.release.title));
    let languages = match &parsed {
        Some(p) => p.languages.iter().map(|l| l.name.to_string()).collect(),
        None => r
            .release
            .languages
            .iter()
            .map(|l| l.name.to_string())
            .collect(),
    };
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
        quality: quality.quality.name.to_string(),
        quality_weight: quality.quality.weight,
        languages,
        release_group: parsed.as_ref().and_then(|p| p.release_group.clone()),
        parsed: parsed.is_some(),
        series_title: parsed.as_ref().map(|p| p.series_title.clone()),
        season_numbers: parsed
            .as_ref()
            .map(|p| p.season_numbers.clone())
            .unwrap_or_default(),
        episode_numbers: parsed
            .as_ref()
            .map(|p| p.episode_numbers.clone())
            .unwrap_or_default(),
        absolute_episode_numbers: parsed
            .as_ref()
            .map(|p| p.absolute_episode_numbers.clone())
            .unwrap_or_default(),
        full_season: parsed.as_ref().is_some_and(|p| p.full_season),
        special: parsed.as_ref().is_some_and(|p| p.special),
        matches_series: parsed.as_ref().is_some_and(|p| {
            let clean = kitsunarr_parser::clean_series_title(&p.series_title);
            let without_year =
                kitsunarr_parser::clean_series_title(&p.series_title_info.title_without_year);
            clean_titles
                .iter()
                .any(|t| *t == clean || *t == without_year)
        }),
    }
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

    let (search, description) = match (q.episode_id, q.season_number) {
        (Some(episode_id), _) => {
            let episodes = series::list_episodes(&state.db, q.series_id, None).await?;
            let e = episodes
                .iter()
                .find(|e| e.id == episode_id)
                .ok_or_else(|| ApiError::NotFound("The episode does not exist.".into()))?;
            let label = format!("S{:02}E{:02}", e.season_number, e.episode_number);
            let daily_date = e
                .air_date
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            match (
                detail.item.series_type,
                daily_date,
                e.absolute_episode_number,
            ) {
                (SeriesType::Daily, Some(date), _) => {
                    (Search::DailyEpisode { date }, format!("{label} ({date})"))
                }
                (SeriesType::Anime, _, Some(absolute)) => (
                    Search::AnimeEpisode {
                        absolute,
                        season: e.season_number,
                        episode: e.episode_number,
                    },
                    format!("{label} (absolute {absolute})"),
                ),
                _ => (
                    Search::SingleEpisode {
                        season: e.season_number,
                        episode: e.episode_number,
                    },
                    label,
                ),
            }
        }
        (None, Some(season)) => {
            let search = if anime {
                Search::AnimeSeason {
                    season,
                    season_scene_titles: Vec::new(),
                }
            } else {
                Search::Season { season }
            };
            (search, format!("Season {season}"))
        }
        (None, None) => {
            return Err(ApiError::field(
                "seasonNumber",
                "Choose a season or an episode to search for.",
            ));
        }
    };

    let indexers: Vec<Indexer> = providers::list(state.db.pool(), ProviderKind::Indexer)
        .await?
        .into_iter()
        .filter(|p| {
            p.options
                .get("enableInteractiveSearch")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        })
        .filter_map(|p| {
            Some(Indexer {
                id: p.id,
                name: p.name.clone(),
                dialect: dialect(&p.implementation)?,
                settings: Settings::from_map(&p.settings).ok()?,
            })
        })
        .collect();

    let client = &state.indexers;
    let outcomes = futures::future::join_all(indexers.iter().map(|indexer| {
        let search = &search;
        let criteria = &criteria;
        async move {
            let caps = client.capabilities(indexer).await?;
            let chain = client.generator(indexer, &caps).search(criteria, search);
            client.fetch(indexer, &chain, false).await
        }
    }))
    .await;

    let clean_titles: Vec<String> = criteria
        .scene_titles
        .iter()
        .map(|t| kitsunarr_parser::clean_series_title(t))
        .collect();
    let mut releases = Vec::new();
    let mut failures = Vec::new();
    for (indexer, outcome) in indexers.iter().zip(outcomes) {
        match outcome {
            Ok(found) => releases.extend(found.into_iter().map(|r| to_item(r, &clean_titles))),
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
    // This series first, then best quality, then newest (the decision engine will refine this).
    releases.sort_by(|a, b| {
        b.matches_series
            .cmp(&a.matches_series)
            .then_with(|| b.quality_weight.cmp(&a.quality_weight))
            .then_with(|| b.publish_date.cmp(&a.publish_date))
    });
    Ok(Json(ReleaseSearchResult {
        releases,
        failures,
        searched_indexers: indexers.len(),
        description,
    }))
}
