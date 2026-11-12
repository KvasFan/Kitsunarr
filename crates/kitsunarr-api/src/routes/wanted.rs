//! Wanted episodes (Sonarr's `MissingController`, `CutoffController`) and searching for them
//! (`EpisodeSearchService`: `EpisodeSearch`, `MissingEpisodeSearch`, `CutoffUnmetEpisodeSearch`).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use kitsunarr_db::library::wanted::{self, BelowCutoff, SearchTarget, Wanted};
use kitsunarr_db::quality;
use kitsunarr_decision::ProfileView;
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::search::ReleaseQuery;
use kitsunarr_domain::api::settings::QualityProfileItem;
use kitsunarr_domain::api::wanted::{
    WantedEpisode, WantedList, WantedQuery, WantedSearchRequest, WantedSearchStarted,
};
use kitsunarr_domain::{Page, PageRequest};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::search::{self, SearchOptions};

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn item_qualities(item: &QualityProfileItem) -> Vec<i32> {
    match (&item.quality_id, &item.group) {
        (Some(q), _) => vec![*q],
        (None, Some(g)) => g.quality_ids.clone(),
        (None, None) => Vec::new(),
    }
}

/// `EpisodeCutoffService`: per profile, the qualities ranked before its cutoff (the first
/// allowed quality when upgrades are off).
async fn below_cutoff(state: &AppState) -> ApiResult<Vec<BelowCutoff>> {
    Ok(quality::list_profiles(state.db.pool())
        .await?
        .iter()
        .filter_map(|p| {
            let view = ProfileView(p);
            let cutoff = if p.upgrade_allowed {
                p.cutoff
            } else {
                view.first_allowed_quality()?
            };
            let index = view.index_of(cutoff, false).index;
            let quality_ids: Vec<i32> = p
                .items
                .iter()
                .take(index)
                .flat_map(item_qualities)
                .collect();
            Some(BelowCutoff {
                profile_id: p.id,
                quality_ids,
            })
        })
        .collect())
}

fn paging(q: &WantedQuery) -> PageRequest {
    PageRequest {
        page: q.page,
        page_size: q.page_size,
        sort_direction: q.sort_direction,
        ..Default::default()
    }
}

/// Aired monitored episodes without a file.
#[utoipa::path(get, path = "/wanted/missing", tag = "wanted", params(WantedQuery),
    responses((status = 200, body = Page<WantedEpisode>)))]
pub async fn missing(
    State(state): State<AppState>,
    Query(q): Query<WantedQuery>,
) -> ApiResult<Json<Page<WantedEpisode>>> {
    let now = now();
    Ok(Json(
        wanted::page(
            &state.db,
            Wanted::Missing { now: &now },
            q.monitored.unwrap_or(true),
            &paging(&q),
        )
        .await?,
    ))
}

/// Monitored episodes whose file is below the quality profile's cutoff.
#[utoipa::path(get, path = "/wanted/cutoff", tag = "wanted", params(WantedQuery),
    responses((status = 200, body = Page<WantedEpisode>)))]
pub async fn cutoff_unmet(
    State(state): State<AppState>,
    Query(q): Query<WantedQuery>,
) -> ApiResult<Json<Page<WantedEpisode>>> {
    let below = below_cutoff(&state).await?;
    Ok(Json(
        wanted::page(
            &state.db,
            Wanted::CutoffUnmet { below: &below },
            q.monitored.unwrap_or(true),
            &paging(&q),
        )
        .await?,
    ))
}

/// Episodes grouped for searching: one group per series and season, oldest search first.
fn groups(targets: Vec<SearchTarget>) -> Vec<Vec<SearchTarget>> {
    let mut groups: Vec<Vec<SearchTarget>> = Vec::new();
    for t in targets {
        match groups
            .iter_mut()
            .find(|g| g[0].series_id == t.series_id && g[0].season_number == t.season_number)
        {
            Some(g) => g.push(t),
            None => groups.push(vec![t]),
        }
    }
    // `OrderBy(Episodes.Min(LastSearchTime ?? MinValue))`, stable; never searched first.
    groups.sort_by_cached_key(|g| {
        g.iter()
            .map(|t| t.last_search_at.clone().unwrap_or_default())
            .min()
    });
    groups
}

/// `SearchForBulkEpisodes`: a season search for groups of several episodes, an episode search
/// otherwise, each followed by grabbing its approved releases. Returns the summary.
pub(crate) async fn bulk_search(
    state: &AppState,
    groups: Vec<Vec<SearchTarget>>,
    monitored_only: bool,
    user_invoked: bool,
    complete_series_first: bool,
) -> String {
    let episodes: usize = groups.iter().map(Vec::len).sum();
    let (mut grabbed, mut failed) = (Vec::new(), Vec::new());
    // Tanukarr's missing searches: a complete-series pack first for a series missing more than
    // one regular season, or episodes of a single-season series; the season and episode
    // searches then skip what a grabbed pack covers.
    let covered = if complete_series_first {
        complete_series_step(
            state,
            &groups,
            monitored_only,
            user_invoked,
            &mut grabbed,
            &mut failed,
        )
        .await
    } else {
        std::collections::HashSet::new()
    };
    for group in &groups {
        let group: Vec<SearchTarget> = group
            .iter()
            .filter(|t| !covered.contains(&t.episode_id))
            .cloned()
            .collect();
        if group.is_empty() {
            continue;
        }
        let first = &group[0];
        let ids: Vec<i64> = group.iter().map(|t| t.episode_id).collect();
        let season = group.len() > 1;
        let q = ReleaseQuery {
            series_id: first.series_id,
            season_number: season.then_some(first.season_number as i32),
            episode_id: (!season).then_some(first.episode_id),
            complete_series: false,
            season_packs_only: false,
        };
        let options = SearchOptions {
            automatic: true,
            monitored_only: season && monitored_only,
            episodes: season.then_some(ids.as_slice()),
            user_invoked,
        };
        let run = match search::run_search(state, &q, options).await {
            Ok(run) => run,
            Err(e) => {
                tracing::warn!(series_id = first.series_id, error = %e.message(), "wanted search failed");
                failed.push(e.message());
                continue;
            }
        };
        let (g, f) =
            search::process_decisions(state, &run, first.series_id, &mut Vec::new(), "Search")
                .await;
        // A search the server started keeps what a delay profile holds back as pending; any
        // grab ends the pending releases of its episodes.
        crate::rss::keep_pending(state, first.series_id, &run).await;
        grabbed.extend(g.into_iter().map(|g| g.title));
        failed.extend(f.into_iter().map(|f| format!("{}: {}", f.title, f.message)));
    }
    let mut summary = format!(
        "Searched {episodes} episodes ({} searches): {} grabbed",
        groups.len(),
        grabbed.len()
    );
    if !grabbed.is_empty() {
        summary.push_str(&format!(" ({})", grabbed.join(", ")));
    }
    if !failed.is_empty() {
        summary.push_str(&format!(". Problems: {}", failed.join("; ")));
    }
    summary.push('.');
    summary
}

/// The complete-series step of missing searches (`EpisodeSearchService`, Tanukarr): returns the
/// episodes grabbed packs cover.
async fn complete_series_step(
    state: &AppState,
    groups: &[Vec<SearchTarget>],
    monitored_only: bool,
    user_invoked: bool,
    grabbed: &mut Vec<String>,
    failed: &mut Vec<String>,
) -> std::collections::HashSet<i64> {
    let mut covered = std::collections::HashSet::new();
    let mut seasons: std::collections::BTreeMap<i64, std::collections::BTreeSet<i64>> =
        std::collections::BTreeMap::new();
    for t in groups.iter().flatten().filter(|t| t.season_number > 0) {
        seasons
            .entry(t.series_id)
            .or_default()
            .insert(t.season_number);
    }
    for (series_id, missing) in seasons {
        let regular = kitsunarr_db::library::series::list_episodes(&state.db, series_id, None)
            .await
            .map(|list| {
                list.iter()
                    .filter(|e| e.season_number > 0)
                    .map(|e| e.season_number)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            })
            .unwrap_or(0);
        if missing.len() < 2 && !(missing.len() == 1 && regular == 1) {
            continue;
        }
        let q = ReleaseQuery {
            series_id,
            season_number: None,
            episode_id: None,
            complete_series: true,
            season_packs_only: false,
        };
        let options = SearchOptions {
            automatic: true,
            monitored_only,
            episodes: None,
            user_invoked,
        };
        let run = match search::run_search(state, &q, options).await {
            Ok(run) => run,
            Err(e) => {
                // No torrent indexer, for instance: the season searches still run.
                tracing::info!(series_id, error = %e.message(), "complete-series search unavailable");
                continue;
            }
        };
        let (g, f) =
            search::process_decisions(state, &run, series_id, &mut Vec::new(), "Search").await;
        crate::rss::keep_pending(state, series_id, &run).await;
        for grab in &g {
            if let Some(d) = run
                .decisions
                .iter()
                .find(|d| d.approved() && run.releases[d.release].release.title == grab.title)
            {
                covered.extend(d.episodes.iter().map(|e| e.id));
            }
        }
        grabbed.extend(g.into_iter().map(|g| g.title));
        failed.extend(f.into_iter().map(|f| format!("{}: {}", f.title, f.message)));
    }
    covered
}

/// `MissingEpisodeSearch` for one series (after adding it), in the background: waits for any
/// running wanted search, then searches the series' missing monitored episodes outside the
/// queue and reports with a notification.
pub(crate) fn search_missing_of_series(state: &AppState, series_id: i64, user_invoked: bool) {
    let state = state.clone();
    tokio::spawn(async move {
        // Tanukarr also suppresses "add and search" while automation is paused.
        if crate::library_tools::paused(&state).await {
            tracing::info!(series_id, "{}", crate::library_tools::PAUSED);
            return;
        }
        let _guard = state.wanted_search.clone().lock_owned().await;
        let now = now();
        let targets = match wanted::all(&state.db, Wanted::Missing { now: &now }, true).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(series_id, error = %e, "could not list missing episodes");
                return;
            }
        };
        let queued = state
            .downloads
            .queued_episode_ids(&state.db)
            .await
            .unwrap_or_default();
        let targets: Vec<SearchTarget> = targets
            .into_iter()
            .filter(|t| t.series_id == series_id && !queued.contains(&t.episode_id))
            .collect();
        if targets.is_empty() {
            return;
        }
        let summary = bulk_search(&state, groups(targets), true, user_invoked, true).await;
        tracing::info!(series_id, %summary, "search for missing episodes completed");
        state.events.publish(ServerEvent::Notice {
            level: "info".into(),
            message: summary,
        });
    });
}

/// Search the indexers for wanted episodes and grab the best releases. The search runs in the
/// background; its outcome arrives as a notification. Episodes already in the queue are skipped.
#[utoipa::path(post, path = "/wanted/search", tag = "wanted", request_body = WantedSearchRequest,
    responses(
        (status = 202, body = WantedSearchStarted),
        (status = 409, body = ApiErrorBody, description = "A wanted search is already running"),
        (status = 422, body = ApiErrorBody)
    ))]
pub async fn search_wanted(
    State(state): State<AppState>,
    Json(req): Json<WantedSearchRequest>,
) -> ApiResult<(StatusCode, Json<WantedSearchStarted>)> {
    let Ok(guard) = state.wanted_search.clone().try_lock_owned() else {
        return Err(ApiError::Conflict(
            "A search for wanted episodes is already running; wait for its notification.".into(),
        ));
    };
    let monitored = req.monitored.unwrap_or(true);
    let targets = if req.episode_ids.is_empty() {
        let now = now();
        let below = below_cutoff(&state).await?;
        let list = match req.list {
            WantedList::Missing => Wanted::Missing { now: &now },
            WantedList::CutoffUnmet => Wanted::CutoffUnmet { below: &below },
        };
        wanted::all(&state.db, list, monitored).await?
    } else {
        wanted::targets(&state.db, &req.episode_ids).await?
    };
    let queued = state
        .downloads
        .queued_episode_ids(&state.db)
        .await
        .map_err(ApiError::internal)?;
    let targets: Vec<SearchTarget> = targets
        .into_iter()
        .filter(|t| !queued.contains(&t.episode_id))
        .collect();
    let groups = groups(targets);
    let episodes: usize = groups.iter().map(Vec::len).sum();
    let started = WantedSearchStarted {
        episodes,
        searches: groups.len(),
        message: if episodes == 0 {
            "Nothing to search: no wanted episode outside the queue.".into()
        } else {
            format!(
                "Searching {episodes} episodes ({} searches); a notification follows.",
                groups.len()
            )
        },
    };
    if episodes > 0 {
        // Chosen episodes are searched as `EpisodeSearch` (monitored or not); whole lists keep
        // their monitored filter.
        let monitored_only = req.episode_ids.is_empty() && monitored;
        // "Search all missing" (monitored) tries complete-series packs first, as Tanukarr.
        let complete_first = monitored_only && matches!(req.list, WantedList::Missing);
        let task_state = state.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let summary =
                bulk_search(&task_state, groups, monitored_only, true, complete_first).await;
            tracing::info!(%summary, "wanted search completed");
            task_state.events.publish(ServerEvent::Notice {
                level: "info".into(),
                message: summary,
            });
        });
    }
    Ok((StatusCode::ACCEPTED, Json(started)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(episode_id: i64, series_id: i64, season: i64, last: Option<&str>) -> SearchTarget {
        SearchTarget {
            episode_id,
            series_id,
            season_number: season,
            last_search_at: last.map(str::to_string),
        }
    }

    #[test]
    fn targets_are_grouped_by_series_and_season_never_searched_first() {
        let g = groups(vec![
            t(1, 1, 1, Some("2026-11-01T00:00:00Z")),
            t(2, 1, 1, Some("2026-11-02T00:00:00Z")),
            t(3, 1, 2, Some("2026-11-03T00:00:00Z")),
            t(4, 2, 1, None),
            t(5, 1, 2, Some("2026-10-01T00:00:00Z")),
        ]);
        let ids: Vec<Vec<i64>> = g
            .iter()
            .map(|g| g.iter().map(|t| t.episode_id).collect())
            .collect();
        assert_eq!(ids, vec![vec![4], vec![3, 5], vec![1, 2]]);
    }
}
