//! Indexer tests and interactive release search.

use axum::Json;
use axum::extract::{Query, State};
use kitsunarr_db::library::series;
use kitsunarr_db::settings as kv;
use kitsunarr_db::{providers, quality, rules};
use kitsunarr_decision::{self as decision, Decision, Engine, SearchKind};
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::library::{EpisodeItem, SeriesDetail};
use kitsunarr_domain::api::search::{
    AutomaticGrab, AutomaticGrabFailure, AutomaticSearchResult, IndexerFailure, IndexerTestRequest,
    IndexerTestResult, ReleaseItem, ReleaseQuery, ReleaseSearchResult,
};
use kitsunarr_domain::api::settings::{
    IndexerOptionsSettings, MediaManagementSettings, ProviderKind,
};
use kitsunarr_domain::quality::QualityModel;
use kitsunarr_domain::{DownloadProtocol, ExternalIdKind, SeriesType};
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
pub(crate) fn describe(e: &IndexerError) -> String {
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

/// The searches to run (each with its decision kind), their description and the requested
/// episodes.
type PlannedSearch = (Vec<(Search, SearchKind)>, String, Vec<i64>);

/// Tanukarr's `CompleteSeriesSearch`: every regular episode; the series' first three clean
/// titles, `Title S01-Sxx` and `Title complete` queries, and in interactive searches one
/// season-pack query per season.
fn complete_series_search(
    scene_titles: &[String],
    episodes: &[EpisodeItem],
    automatic: bool,
) -> ApiResult<PlannedSearch> {
    let regular: Vec<&EpisodeItem> = episodes.iter().filter(|e| e.season_number > 0).collect();
    if regular.is_empty() {
        return Err(ApiError::field(
            "completeSeries",
            "The series has no regular episodes to search for.",
        ));
    }
    let mut query_titles: Vec<String> = Vec::new();
    for t in scene_titles
        .iter()
        .map(|t| clean_scene_title(t).replace('+', " "))
    {
        if !t.is_empty()
            && !query_titles
                .iter()
                .any(|q| q.to_lowercase() == t.to_lowercase())
        {
            query_titles.push(t);
        }
    }
    query_titles.truncate(3);
    let mut seasons: Vec<i32> = regular.iter().map(|e| e.season_number).collect();
    seasons.sort_unstable();
    seasons.dedup();
    let last = seasons.last().copied().unwrap_or(1);
    let mut packs: Vec<String> = Vec::new();
    for t in &query_titles {
        if last > 1 {
            packs.push(format!("{t} S01-S{last:02}"));
        }
        packs.push(format!("{t} complete"));
    }
    if !automatic && let Some(first) = query_titles.first() {
        packs.extend(seasons.iter().map(|s| format!("{first} S{s:02}")));
    }
    Ok((
        vec![(
            Search::CompleteSeries {
                query_titles,
                pack_query_titles: packs,
            },
            SearchKind::CompleteSeries {
                include_season_packs: !automatic,
            },
        )],
        "Complete series".into(),
        regular.iter().map(|e| e.id).collect(),
    ))
}

/// Indexer settings the decisions need, by indexer id.
pub(crate) type Extras = std::collections::HashMap<i64, IndexerExtras>;

/// The indexers with `option` (`enableRss`, `enableAutomaticSearch`, `enableInteractiveSearch`)
/// on. Indexers that cannot be used are reported, not skipped silently.
pub(crate) async fn usable_indexers(
    state: &AppState,
    option: &str,
) -> ApiResult<(Vec<Indexer>, Extras, Vec<IndexerFailure>)> {
    let mut indexers = Vec::new();
    let mut extras = Extras::new();
    let mut failures = Vec::new();
    for p in providers::list(state.db.pool(), ProviderKind::Indexer).await? {
        let enabled = p
            .options
            .get(option)
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enabled {
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
    Ok((indexers, extras, failures))
}

/// A finished search: its decisions, best first, over `releases`.
pub(crate) struct SearchRun {
    pub decisions: Vec<Decision>,
    pub releases: Vec<IndexerRelease>,
    pub failures: Vec<IndexerFailure>,
    pub searched_indexers: usize,
    pub description: String,
    pub warnings: Vec<String>,
    pub series: decision::Series,
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
    let run = run_search(&state, &q, SearchOptions::INTERACTIVE).await?;
    cache(&state, &run, q.series_id);
    let releases = run
        .decisions
        .iter()
        .map(|d| to_item(run.releases[d.release].clone(), d, q.series_id))
        .collect();
    Ok(Json(ReleaseSearchResult {
        releases,
        failures: run.failures,
        searched_indexers: run.searched_indexers,
        description: run.description,
        warnings: run.warnings,
    }))
}

/// Search the indexers enabled for automatic search for a season or an episode and grab the
/// best approved releases, as Sonarr's `EpisodeSearch`/`SeasonSearch` commands do.
#[utoipa::path(post, path = "/releases/search", tag = "search", request_body = ReleaseQuery,
    responses(
        (status = 200, body = AutomaticSearchResult),
        (status = 404, body = ApiErrorBody),
        (status = 422, body = ApiErrorBody)
    ))]
pub async fn automatic_search(
    State(state): State<AppState>,
    Json(q): Json<ReleaseQuery>,
) -> ApiResult<Json<AutomaticSearchResult>> {
    let run = run_search(
        &state,
        &q,
        SearchOptions {
            automatic: true,
            monitored_only: q.episode_id.is_none(),
            episodes: None,
        },
    )
    .await?;
    let (grabbed, grab_failures) =
        process_decisions(&state, &run, q.series_id, &mut Vec::new(), "Search").await;
    Ok(Json(AutomaticSearchResult {
        description: run.description.clone(),
        searched_indexers: run.searched_indexers,
        failures: run.failures.clone(),
        warnings: run.warnings.clone(),
        found: run.releases.len(),
        approved: run.decisions.iter().filter(|d| d.approved()).count(),
        grabbed,
        grab_failures,
    }))
}

/// `ProcessDownloadDecisions.ProcessDecisions`: the approved releases, best first, each grabbed
/// unless an episode of it was already grabbed by this search; a protocol whose download client
/// is unavailable is skipped from then on.
pub(crate) async fn process_decisions(
    state: &AppState,
    run: &SearchRun,
    series_id: i64,
    unavailable: &mut Vec<DownloadProtocol>,
    release_source: &'static str,
) -> (Vec<AutomaticGrab>, Vec<AutomaticGrabFailure>) {
    let mut grabbed = Vec::new();
    let mut failures = Vec::new();
    let mut taken: Vec<i64> = Vec::new();
    for d in run.decisions.iter().filter(|d| d.approved()) {
        let mut cached = cached_release(run, d, series_id);
        cached.release_source = release_source;
        if cached.episode_ids.is_empty() || cached.episode_ids.iter().any(|e| taken.contains(e)) {
            continue;
        }
        let protocol = cached.release.release.protocol;
        if unavailable.contains(&protocol) {
            continue;
        }
        let title = cached.release.release.title.clone();
        let target = match crate::routes::grab::pick_client(state, &cached).await {
            Ok(t) => t,
            Err(e) => {
                unavailable.push(protocol);
                failures.push(AutomaticGrabFailure {
                    title,
                    message: e.message(),
                });
                continue;
            }
        };
        match crate::routes::grab::send(state, &cached, target).await {
            Ok(r) => {
                taken.extend(&cached.episode_ids);
                grabbed.push(AutomaticGrab {
                    title,
                    download_client: r.download_client,
                    download_id: r.download_id,
                });
            }
            Err(e) => {
                tracing::warn!(release = %title, error = %e.message(), "automatic grab failed");
                failures.push(AutomaticGrabFailure {
                    title,
                    message: e.message(),
                });
            }
        }
    }
    (grabbed, failures)
}

/// How a search runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SearchOptions<'a> {
    /// Indexers enabled for automatic search (else interactive search).
    pub automatic: bool,
    /// Only monitored episodes are wanted (automatic season searches, wanted lists).
    pub monitored_only: bool,
    /// For a season search: the requested episodes (default: the whole season).
    pub episodes: Option<&'a [i64]>,
}

impl SearchOptions<'_> {
    pub(crate) const INTERACTIVE: Self = Self {
        automatic: false,
        monitored_only: false,
        episodes: None,
    };
}

/// Searches the indexers enabled for interactive or automatic search that share a tag with the
/// series (or have none), decides on every release and records the episodes' last search time
/// (`ReleaseSearchService.Dispatch`).
pub(crate) async fn run_search(
    state: &AppState,
    q: &ReleaseQuery,
    options: SearchOptions<'_>,
) -> ApiResult<SearchRun> {
    let automatic = options.automatic;
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
    if q.season_packs_only
        && (q.season_number.is_none_or(|s| s <= 0) || q.episode_id.is_some() || q.complete_series)
    {
        return Err(ApiError::field(
            "seasonPacksOnly",
            "A season-pack search takes a season (above 0) and nothing else.",
        ));
    }
    let (searches, description, requested): PlannedSearch = if q.complete_series {
        if q.episode_id.is_some() || q.season_number.is_some() {
            return Err(ApiError::field(
                "completeSeries",
                "A complete-series search takes no season or episode.",
            ));
        }
        complete_series_search(&criteria.scene_titles, &episodes, automatic)?
    } else {
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
                let requested = match options.episodes {
                    Some(ids) => ids.to_vec(),
                    None => episodes
                        .iter()
                        .filter(|e| e.season_number == season)
                        .map(|e| e.id)
                        .collect(),
                };
                (vec![search], format!("Season {season}"), requested)
            }
            (None, None) => {
                return Err(ApiError::field(
                    "seasonNumber",
                    "Choose a season or an episode to search for.",
                ));
            }
        }
    };

    let option = if automatic {
        "enableAutomaticSearch"
    } else {
        "enableInteractiveSearch"
    };
    let (mut indexers, extras, mut failures) = usable_indexers(state, option).await?;
    indexers.retain(|i| {
        extras.get(&i.id).is_none_or(|x| {
            x.tags.is_empty() || x.tags.iter().any(|t| detail.item.tags.contains(t))
        })
    });
    // Tanukarr's `Dispatch`: complete-series searches use torrent indexers only.
    if q.complete_series {
        indexers.retain(|i| i.dialect == Dialect::Torznab);
        if indexers.is_empty() {
            return Err(ApiError::field(
                "completeSeries",
                "No torrent indexers are enabled for complete-series search.",
            ));
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
                tracing::warn!(indexer = %indexer.name, error = %e, "search failed");
                failures.push(IndexerFailure {
                    indexer_id: indexer.id,
                    indexer: indexer.name.clone(),
                    message: describe(&e),
                });
            }
        }
    }
    // `UpdateLastSearchTime` when at least one indexer was searched.
    if !indexers.is_empty() {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        kitsunarr_db::library::wanted::set_last_search(&state.db, &requested, &now).await?;
    }

    let inputs = DecisionInputs::load(state, &detail, &episodes).await?;
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
            monitored_episodes_only: options.monitored_only,
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
    let mut decisions = engine.prioritize(Engine::dedupe(decisions, &all), &all);
    // Tanukarr's `SeasonPackSearch`: full-season releases only.
    if q.season_packs_only {
        decisions.retain(|d| d.parsed.as_ref().is_some_and(|p| p.full_season));
    }
    let run = SearchRun {
        decisions,
        releases: all_meta,
        failures,
        searched_indexers: indexers.len(),
        description,
        warnings: inputs.warnings,
        series: inputs.series,
    };
    crate::library_tools::record_search(state, &run, q.series_id, automatic).await;
    Ok(run)
}

/// Keeps a search's releases for grabbing: download links stay on the server.
pub(crate) fn cache(state: &AppState, run: &SearchRun, series_id: i64) {
    for d in &run.decisions {
        let mut cached = cached_release(run, d, series_id);
        cached.release_source = "InteractiveSearch";
        state.releases.insert(cached);
    }
}

/// The grab-cache entry for one decision.
pub(crate) fn cached_release(
    run: &SearchRun,
    d: &Decision,
    series_id: i64,
) -> crate::routes::grab::CachedRelease {
    let recent_cutoff = (chrono::Utc::now() - chrono::Duration::days(14))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|d| d.and_utc());
    let episodes: Vec<&decision::Episode> = d
        .episodes
        .iter()
        .filter_map(|m| run.series.episodes.iter().find(|e| e.id == m.id))
        .collect();
    crate::routes::grab::CachedRelease {
        release: run.releases[d.release].clone(),
        series_id,
        episode_ids: episodes.iter().map(|e| e.id).collect(),
        quality: QualityModel {
            quality_id: d.quality.quality.id,
            revision: d.quality.revision,
        },
        languages: d.languages.iter().map(|l| l.id).collect(),
        custom_format_score: d.custom_format_score,
        release_group: d.parsed.as_ref().and_then(|p| p.release_group.clone()),
        release_type: kitsunarr_library::decision_input::release_type_name(
            d.parsed
                .as_ref()
                .map_or(kitsunarr_parser::ReleaseType::Unknown, |p| p.release_type()),
        ),
        recent: episodes
            .iter()
            .any(|e| matches!((e.air_date_utc, recent_cutoff), (Some(a), Some(c)) if a >= c)),
        release_source: "Search",
        manual_mapping: false,
        cached_at: std::time::Instant::now(),
    }
}

/// Indexer settings the decisions read.
pub(crate) struct IndexerExtras {
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

pub(crate) fn decision_release(
    r: &IndexerRelease,
    extras: Option<&IndexerExtras>,
) -> decision::Release {
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
        indexer: r.indexer.clone(),
        info_hash: rel
            .info_hash
            .clone()
            .filter(|h| !h.trim().is_empty())
            .or_else(|| {
                rel.magnet_url
                    .as_deref()
                    .and_then(kitsunarr_downloaders::magnet_info_hash)
            }),
    }
}

/// The series, library, custom formats and settings a search's decisions need.
pub(crate) struct DecisionInputs {
    pub series: decision::Series,
    pub library: Vec<decision::LibrarySeries>,
    pub formats: Vec<decision::CompiledFormat>,
    pub settings: decision::Settings,
    pub warnings: Vec<String>,
}

impl DecisionInputs {
    pub(crate) async fn load(
        state: &AppState,
        detail: &SeriesDetail,
        episodes: &[EpisodeItem],
    ) -> ApiResult<Self> {
        let db = &state.db;
        let series = kitsunarr_library::decision_input::series_model(db, detail, episodes).await?;
        let library = kitsunarr_library::decision_input::library_titles(db).await?;
        let (formats, warnings) =
            decision::compile_formats(&quality::list_custom_formats(db.pool()).await?);
        let media: MediaManagementSettings =
            kv::get_or_default(db.pool(), kv::KEY_MEDIA_MANAGEMENT).await?;
        let options: IndexerOptionsSettings =
            kv::get_or_default(db.pool(), kv::KEY_INDEXER_OPTIONS).await?;
        let tools: kitsunarr_domain::api::settings::LibraryToolsSettings =
            kv::get_or_default(db.pool(), kv::KEY_LIBRARY_TOOLS).await?;
        let settings = decision::Settings {
            last_grabs: kitsunarr_library::decision_input::last_grabs(db, detail.item.id).await?,
            blocklist: kitsunarr_library::decision_input::blocklist(db, detail.item.id).await?,
            pending: kitsunarr_db::pending::list(db.pool())
                .await?
                .into_iter()
                .filter(|p| p.series_id == detail.item.id)
                .filter_map(|p| {
                    let release: serde_json::Value = serde_json::from_str(&p.release).ok()?;
                    Some(decision::PendingRelease {
                        series_id: p.series_id,
                        episode_ids: serde_json::from_str(&p.episode_ids).unwrap_or_default(),
                        publish_date: release["publishDate"].as_str()?.parse().ok()?,
                    })
                })
                .collect(),
            enable_multi_season_releases: options.enable_multi_season_releases,
            match_external_ids: tools.match_external_ids,
            queue: state.downloads.queued_releases(db, detail.item.id).await?,
            quality_definitions: quality::list_definitions(db.pool()).await?,
            propers: media.download_propers_and_repacks,
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
                    usenet_delay: d.usenet_delay,
                    torrent_delay: d.torrent_delay,
                    bypass_if_highest_quality: d.bypass_if_highest_quality,
                    bypass_if_above_custom_format_score: d.bypass_if_above_custom_format_score,
                    minimum_custom_format_score: d.minimum_custom_format_score as i32,
                    order: d.sort_order,
                    tags: d.tags,
                })
                .collect(),
            minimum_age_minutes: i64::from(options.minimum_age),
            retention_days: i64::from(options.retention),
            maximum_size_mb: i64::from(options.maximum_size),
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
