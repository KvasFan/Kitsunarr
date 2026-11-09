//! Indexer tests and interactive release search.

use axum::Json;
use axum::extract::{Query, State};
use kitsunarr_db::library::series;
use kitsunarr_db::providers;
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::library::EpisodeItem;
use kitsunarr_domain::api::search::{
    IndexerFailure, IndexerTestRequest, IndexerTestResult, ReleaseItem, ReleaseQuery,
    ReleaseSearchResult,
};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_domain::{ExternalIdKind, SeriesType};
use kitsunarr_indexers::feed::flags;
use kitsunarr_indexers::request::{clean_scene_title, search_mode};
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

/// `ReleaseSearchService.EpisodeSearch`: the searches for one episode, with a description.
fn episode_searches(
    series_type: SeriesType,
    scene_titles: &[String],
    e: &EpisodeItem,
) -> ApiResult<(Vec<Search>, String)> {
    let label = format!("S{:02}E{:02}", e.season_number, e.episode_number);
    let single = Search::SingleEpisode {
        season: e.season_number,
        episode: e.episode_number,
    };
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
                Search::Special {
                    episode_query_titles: queries,
                },
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
                vec![Search::DailyEpisode { date }],
                format!("{label} ({date})"),
            )
        }
        SeriesType::Anime if e.season_number == 0 && e.absolute_episode_number.is_none() => {
            special()
        }
        SeriesType::Anime => {
            let absolute = e.absolute_episode_number.unwrap_or(0);
            (
                vec![Search::AnimeEpisode {
                    absolute,
                    season: e.season_number,
                    episode: e.episode_number,
                }],
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

    let (searches, description) = match (q.episode_id, q.season_number) {
        (Some(episode_id), _) => {
            let episodes = series::list_episodes(&state.db, q.series_id, None).await?;
            let e = episodes
                .iter()
                .find(|e| e.id == episode_id)
                .ok_or_else(|| ApiError::NotFound("The episode does not exist.".into()))?;
            episode_searches(detail.item.series_type, &criteria.scene_titles, e)?
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
            (vec![search], format!("Season {season}"))
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
            Ok((dialect, settings)) => indexers.push(Indexer {
                id: p.id,
                name: p.name,
                dialect,
                settings,
            }),
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
            for search in searches {
                let chain = generator.search(criteria, search);
                found.extend(client.fetch(indexer, &chain, false).await?);
            }
            Ok::<_, IndexerError>(found)
        }
    }))
    .await;

    let clean_titles: Vec<String> = criteria
        .scene_titles
        .iter()
        .map(|t| kitsunarr_parser::clean_series_title(t))
        .collect();
    let mut releases = Vec::new();
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
    // `DeDupeDecisions`: one entry per GUID (the first indexer's, until the decision engine
    // ranks by rejections and indexer priority).
    let mut seen = std::collections::HashSet::new();
    releases.retain(|r| seen.insert(r.guid.clone()));
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
        episode_searches(kind, &titles, e).unwrap().0
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
