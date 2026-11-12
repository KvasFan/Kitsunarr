//! Release search for films: Tanukarr's `MovieSearch`. Indexers are asked for "Title Year"
//! (up to three titles) on their movie categories (and anime categories for anime films);
//! releases are decided as the film's one feature episode and grabbed into the film's history.

use axum::Json;
use axum::extract::{Path, State};
use kitsunarr_db::library::movies;
use kitsunarr_decision::{self as decision, Engine, SearchKind};
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::search::{AutomaticSearchResult, IndexerFailure, ReleaseSearchResult};
use kitsunarr_domain::{ExternalIdKind, MetadataProvider};
use kitsunarr_indexers::request::search_mode;
use kitsunarr_indexers::{Criteria, IndexerError, IndexerRelease, Search, SeriesIds};

use crate::AppState;
use crate::error::ApiResult;
use crate::routes::search::{
    DecisionInputs, Owner, SearchRun, cached_release, decision_release, describe, record_outcome,
    to_item, usable_indexers,
};

/// Searches the indexers for a film and decides on every release (a search the user started:
/// delay profiles do not apply).
pub(crate) async fn run_movie_search(
    state: &AppState,
    movie_id: i64,
    automatic: bool,
) -> ApiResult<SearchRun> {
    run_movie_search_as(state, movie_id, automatic, true).await
}

/// A film search the server starts (a failed download's redownload, add and search): delay
/// profiles apply, and the releases they hold become pending for the RSS sync.
pub(crate) async fn server_search(state: &AppState, movie_id: i64) {
    match run_movie_search_as(state, movie_id, true, false).await {
        Ok(run) => {
            let r = grab_best(state, &run, movie_id, "Search").await;
            keep_pending(state, movie_id, &run, !r.grabbed.is_empty()).await;
        }
        Err(e) => tracing::warn!(movie = movie_id, error = %e.message(), "the film search failed"),
    }
}

/// `rss::keep_pending` for a film.
async fn keep_pending(state: &AppState, movie_id: i64, run: &SearchRun, grabbed: bool) {
    let db = state.db.pool();
    if grabbed {
        let _ = kitsunarr_db::pending::remove_for_movie(db, movie_id).await;
        return;
    }
    for d in &run.decisions {
        let r = &run.releases[d.release];
        let delayed = !d.rejections.is_empty()
            && d.rejections
                .iter()
                .all(|x| x.reason == decision::RejectionReason::MinimumAgeDelay);
        let result = if delayed {
            kitsunarr_db::pending::upsert_movie(
                db,
                movie_id,
                r.indexer_id,
                &r.release.guid,
                &r.release.title,
                &serde_json::to_string(&crate::rss::StoredRelease::of(r)).unwrap_or_default(),
            )
            .await
        } else if !d.approved() {
            kitsunarr_db::pending::remove(db, r.indexer_id, &r.release.guid).await
        } else {
            Ok(())
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "pending release bookkeeping failed");
        }
    }
}

async fn run_movie_search_as(
    state: &AppState,
    movie_id: i64,
    automatic: bool,
    user_invoked: bool,
) -> ApiResult<SearchRun> {
    let detail = movies::get_detail(&state.db, movie_id).await?;
    let id_of = |kind: ExternalIdKind| {
        detail
            .external_ids
            .iter()
            .find(|e| e.kind == kind)
            .map(|e| e.value.clone())
    };
    let year = detail.item.year.unwrap_or(0);
    let original = movies::original_title(&state.db, movie_id).await?;
    let mut titles = vec![detail.item.title.clone()];
    if let Some(o) = original.clone().filter(|o| !o.trim().is_empty())
        && !titles.iter().any(|t| t.eq_ignore_ascii_case(&o))
    {
        titles.push(o);
    }
    titles.truncate(3);
    let mut query_titles: Vec<String> = titles
        .iter()
        .map(|t| {
            if year > 0 {
                format!("{t} {year}")
            } else {
                t.clone()
            }
        })
        .collect();
    // Tanukarr's collection packs: `{collection} {first year}-{last year}` is searched too.
    let pack = kitsunarr_library::decision_input::collection_pack(&state.db, movie_id).await?;
    if let Some(p) = &pack {
        let first = p.movies.iter().map(|m| m.year).min().unwrap_or(0);
        let last = p.movies.iter().map(|m| m.year).max().unwrap_or(0);
        query_titles.push(format!("{} {first}-{last}", p.title));
    }
    let anime = matches!(
        detail.item.metadata_provider,
        MetadataProvider::AniList | MetadataProvider::MyAnimeList
    );
    let criteria = Criteria {
        ids: SeriesIds {
            tvdb: 0,
            imdb: id_of(ExternalIdKind::Imdb),
            tv_rage: 0,
            tv_maze: 0,
            tmdb: id_of(ExternalIdKind::Tmdb)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        },
        scene_titles: titles.clone(),
        search_mode: search_mode::DEFAULT,
        anime,
    };
    let search = Search::Movie { query_titles };
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
    let client = &state.indexers;
    let outcomes = futures::future::join_all(indexers.iter().map(|indexer| {
        let (criteria, search) = (&criteria, &search);
        async move {
            let caps = client.capabilities(indexer).await?;
            let chain = client.generator(indexer, &caps).search(criteria, search);
            client.fetch(indexer, &chain, false).await
        }
    }))
    .await;
    let mut found: Vec<IndexerRelease> = Vec::new();
    for (indexer, outcome) in indexers.iter().zip(outcomes) {
        record_outcome(state, indexer.id, &outcome).await;
        match outcome {
            Ok(list) => found.extend(list),
            Err(e) => {
                let e: IndexerError = e;
                tracing::warn!(indexer = %indexer.name, error = %e, "movie search failed");
                failures.push(IndexerFailure {
                    indexer_id: indexer.id,
                    indexer: indexer.name.clone(),
                    message: describe(&e),
                });
            }
        }
    }
    if !indexers.is_empty() {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        movies::set_last_search(&state.db, movie_id, &now).await?;
    }
    let model = kitsunarr_library::decision_input::movie_model(&state.db, &detail).await?;
    let inputs = DecisionInputs::with_model(state, model, Owner::Movie(movie_id)).await?;
    let engine = Engine {
        series: &inputs.series,
        library: &inputs.library,
        formats: &inputs.formats,
        settings: &inputs.settings,
    };
    let group: Vec<decision::Release> = found
        .iter()
        .map(|r| decision_release(r, extras.get(&r.indexer_id)))
        .collect();
    let imdb = id_of(ExternalIdKind::Imdb);
    let candidate = kitsunarr_parser::movie::MovieCandidate {
        title: &detail.item.title,
        original_title: original.as_deref(),
        alternate_titles: &[],
        year,
        imdb_id: imdb.as_deref(),
        anime_catalogue: anime,
        collection: pack.as_ref(),
    };
    let criteria = decision::SearchCriteria {
        kind: SearchKind::Special,
        episode_ids: vec![movie_id],
        monitored_episodes_only: automatic,
        user_invoked,
    };
    let decisions = engine.evaluate_movie(&group, &criteria, &candidate);
    let decisions = engine.prioritize(Engine::dedupe(decisions, &group), &group);
    Ok(SearchRun {
        decisions,
        releases: found,
        failures,
        searched_indexers: indexers.len(),
        description: if year > 0 {
            format!("{} ({year})", detail.item.title)
        } else {
            detail.item.title.clone()
        },
        warnings: inputs.warnings,
        series: inputs.series,
    })
}

/// Caches a film search's releases for grabbing.
fn cache(state: &AppState, run: &SearchRun, movie_id: i64, source: &'static str) {
    for d in &run.decisions {
        let mut cached = cached_release(run, d, 0);
        cached.movie_id = Some(movie_id);
        cached.episode_ids = Vec::new();
        cached.release_source = source;
        state.releases.insert(cached);
    }
}

/// Searches every indexer enabled for interactive search for a film.
#[utoipa::path(get, path = "/movies/{id}/releases", tag = "search",
    params(("id" = i64, Path)),
    responses((status = 200, body = ReleaseSearchResult), (status = 404, body = ApiErrorBody)))]
pub async fn search_movie_releases(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ReleaseSearchResult>> {
    let run = run_movie_search(&state, id, false).await?;
    cache(&state, &run, id, "InteractiveSearch");
    let releases = run
        .decisions
        .iter()
        .map(|d| to_item(run.releases[d.release].clone(), d, 0))
        .collect();
    Ok(Json(ReleaseSearchResult {
        releases,
        failures: run.failures,
        searched_indexers: run.searched_indexers,
        description: run.description,
        warnings: run.warnings,
    }))
}

/// Searches the indexers enabled for automatic search for a film and grabs the best approved
/// release.
#[utoipa::path(post, path = "/movies/{id}/search", tag = "search",
    params(("id" = i64, Path)),
    responses((status = 200, body = AutomaticSearchResult), (status = 404, body = ApiErrorBody)))]
pub async fn automatic_movie_search(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<AutomaticSearchResult>> {
    let run = run_movie_search(&state, id, true).await?;
    Ok(Json(grab_best(&state, &run, id, "Search").await))
}

/// Monitored films without a file that are available.
#[utoipa::path(get, path = "/wanted/movies", tag = "wanted",
    responses((status = 200, body = Vec<kitsunarr_domain::api::wanted::WantedMovie>)))]
pub async fn missing_movies(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<kitsunarr_domain::api::wanted::WantedMovie>>> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    Ok(Json(movies::missing(&state.db, &today).await?))
}

/// Searches for missing films (all of them, or the given ones) in the background, grabbing
/// each one's best accepted release.
#[utoipa::path(post, path = "/wanted/movies/search", tag = "wanted",
    request_body = kitsunarr_domain::api::wanted::WantedMovieSearchRequest,
    responses((status = 202, body = kitsunarr_domain::api::wanted::WantedSearchStarted)))]
pub async fn search_missing_movies(
    State(state): State<AppState>,
    Json(req): Json<kitsunarr_domain::api::wanted::WantedMovieSearchRequest>,
) -> ApiResult<(
    axum::http::StatusCode,
    Json<kitsunarr_domain::api::wanted::WantedSearchStarted>,
)> {
    let ids: Vec<i64> = if req.movie_ids.is_empty() {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        movies::missing(&state.db, &today)
            .await?
            .into_iter()
            .map(|m| m.id)
            .collect()
    } else {
        req.movie_ids.clone()
    };
    let count = ids.len();
    let task_state = state.clone();
    tokio::spawn(async move {
        let mut grabbed = 0usize;
        for id in ids {
            match run_movie_search(&task_state, id, true).await {
                Ok(run) => {
                    grabbed += grab_best(&task_state, &run, id, "Search")
                        .await
                        .grabbed
                        .len();
                }
                Err(e) => {
                    tracing::warn!(movie = id, error = %e.message(), "the film search failed")
                }
            }
        }
        tracing::info!(searched = count, grabbed, "missing film search finished");
    });
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(kitsunarr_domain::api::wanted::WantedSearchStarted {
            episodes: count,
            searches: count,
            message: if count == 0 {
                "Nothing to search: no missing film.".into()
            } else {
                format!("Searching {count} films.")
            },
        }),
    ))
}

/// Grabs the best approved release of a film search.
pub(crate) async fn grab_best(
    state: &AppState,
    run: &SearchRun,
    movie_id: i64,
    source: &'static str,
) -> AutomaticSearchResult {
    let mut grabbed = Vec::new();
    let mut grab_failures = Vec::new();
    if let Some(d) = run.decisions.iter().find(|d| d.approved()) {
        let mut cached = cached_release(run, d, 0);
        cached.movie_id = Some(movie_id);
        cached.episode_ids = Vec::new();
        cached.release_source = source;
        let title = cached.release.release.title.clone();
        let result = match crate::routes::grab::pick_client(state, &cached).await {
            Ok(target) => crate::routes::grab::send(state, &cached, target).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(r) => grabbed.push(kitsunarr_domain::api::search::AutomaticGrab {
                title,
                download_client: r.download_client,
                download_id: r.download_id,
            }),
            Err(e) => grab_failures.push(kitsunarr_domain::api::search::AutomaticGrabFailure {
                title,
                message: e.message(),
            }),
        }
    }
    AutomaticSearchResult {
        description: run.description.clone(),
        searched_indexers: run.searched_indexers,
        failures: run.failures.clone(),
        warnings: run.warnings.clone(),
        found: run.releases.len(),
        approved: run.decisions.iter().filter(|d| d.approved()).count(),
        grabbed,
        grab_failures,
    }
}
