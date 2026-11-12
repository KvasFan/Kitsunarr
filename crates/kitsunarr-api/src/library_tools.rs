//! Tanukarr's Library tools: automation pause and the recent-searches diagnostics.
//!
//! While automation is paused, RSS sync, scheduled import list syncs and the searches Kitsunarr
//! starts by itself (redownloads after failures, "search for missing" after adding a title,
//! titles added by lists) do nothing. Searches and grabs the user starts, imports of current
//! downloads, refreshes and disk scans continue.

use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::{LibraryToolsSettings, RejectionCount, SearchActivity};

use crate::AppState;

/// Searches kept for "Recent searches".
const HISTORY: usize = 100;
/// Rejection messages kept per search.
const TOP_REJECTIONS: usize = 12;

/// Tanukarr's message for skipped automatic work.
pub const PAUSED: &str = "Automation paused; manual actions remain available.";

pub async fn settings(state: &AppState) -> LibraryToolsSettings {
    kv::get_or_default(state.db.pool(), kv::KEY_LIBRARY_TOOLS)
        .await
        .unwrap_or_default()
}

pub async fn paused(state: &AppState) -> bool {
    settings(state).await.automation_paused
}

/// Records a search (newest first, at most 100).
pub(crate) async fn record_search(
    state: &AppState,
    run: &crate::routes::search::SearchRun,
    series_id: i64,
    automatic: bool,
) {
    let mut counts: Vec<RejectionCount> = Vec::new();
    for d in &run.decisions {
        for r in &d.rejections {
            match counts.iter_mut().find(|c| c.message == r.message) {
                Some(c) => c.count += 1,
                None => counts.push(RejectionCount {
                    message: r.message.clone(),
                    count: 1,
                }),
            }
        }
    }
    counts.sort_by(|a, b| b.count.cmp(&a.count).then(a.message.cmp(&b.message)));
    counts.truncate(TOP_REJECTIONS);
    let mut failures: Vec<String> = run
        .failures
        .iter()
        .map(|f| format!("{}: {}", f.indexer, f.message))
        .collect();
    failures.dedup();
    let activity = SearchActivity {
        date: kitsunarr_db::now(),
        series_id,
        query: format!("{}: {}", run.series.title, run.description),
        automatic,
        found: run.decisions.len() as i64,
        accepted: run.decisions.iter().filter(|d| d.approved()).count() as i64,
        indexers: run.searched_indexers as i64,
        rejections: counts,
        provider_failures: failures,
    };
    let mut history: Vec<SearchActivity> =
        kv::get_or_default(state.db.pool(), kv::KEY_SEARCH_HISTORY)
            .await
            .unwrap_or_default();
    history.insert(0, activity);
    history.truncate(HISTORY);
    if let Err(e) = kv::set(state.db.pool(), kv::KEY_SEARCH_HISTORY, &history).await {
        tracing::debug!(error = %e, "the search could not be recorded");
    }
}

pub async fn recent_searches(state: &AppState) -> Vec<SearchActivity> {
    kv::get_or_default(state.db.pool(), kv::KEY_SEARCH_HISTORY)
        .await
        .unwrap_or_default()
}
