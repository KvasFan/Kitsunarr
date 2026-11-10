//! RSS sync: Sonarr's `RssSyncService`, `FetchAndParseRssService` and `GetRssDecision`.
//!
//! Each run reads the newest releases of every indexer with RSS enabled, maps each release to a
//! library series, decides on it with the RSS rules (monitored episodes, history, delay profiles,
//! propers) and grabs the approved ones, best first, as an automatic search does.

use std::collections::HashMap;

use kitsunarr_db::library::series;
use kitsunarr_db::settings as kv;
use kitsunarr_db::{Database, DbResult, tasks};
use kitsunarr_decision::{self as decision, Engine, SearchCriteria};
use kitsunarr_domain::api::search::IndexerFailure;
use kitsunarr_domain::api::settings::IndexerOptionsSettings;
use kitsunarr_indexers::IndexerRelease;

use crate::AppState;
use crate::routes::search::{self, DecisionInputs, SearchRun};

/// The scheduled task's name.
pub const TASK: &str = "RssSync";

/// Applies the "RSS sync interval" setting to the scheduled task: 0 turns RSS sync off;
/// otherwise every 10 to 120 minutes (Sonarr's limits).
pub async fn sync_interval(db: &Database) -> DbResult<()> {
    let options: IndexerOptionsSettings =
        kv::get_or_default(db.pool(), kv::KEY_INDEXER_OPTIONS).await?;
    let minutes = i64::from(options.rss_sync_interval);
    let enabled = minutes > 0;
    let interval = if enabled { minutes.clamp(10, 120) } else { 15 };
    tasks::set_interval(db.pool(), TASK, interval, enabled).await?;
    Ok(())
}

/// Series loaded for decisions, by id (`None`: the series could not be loaded).
type Loaded = HashMap<i64, Option<DecisionInputs>>;

async fn load<'a>(
    state: &AppState,
    loaded: &'a mut Loaded,
    series_id: i64,
) -> Option<&'a DecisionInputs> {
    if let std::collections::hash_map::Entry::Vacant(slot) = loaded.entry(series_id) {
        let inputs = async {
            let detail = series::get_detail(&state.db, series_id).await?;
            let episodes = series::list_episodes(&state.db, series_id, None).await?;
            DecisionInputs::load(state, &detail, &episodes).await
        }
        .await;
        if let Err(e) = &inputs {
            tracing::warn!(series_id, error = %e.message(), "RSS sync could not load a series");
        }
        slot.insert(inputs.ok());
    }
    loaded.get(&series_id).and_then(Option::as_ref)
}

/// One RSS sync. Returns the summary for the task history; fails when no indexer could be read.
pub async fn sync(state: &AppState) -> Result<String, String> {
    let (indexers, extras, mut failures) = search::usable_indexers(state, "enableRss")
        .await
        .map_err(|e| e.message())?;
    if indexers.is_empty() && failures.is_empty() {
        return Ok("No indexer has RSS enabled".into());
    }

    let client = &state.indexers;
    let outcomes = futures::future::join_all(indexers.iter().map(|indexer| async move {
        let caps = client.capabilities(indexer).await?;
        let chain = client.generator(indexer, &caps).recent();
        client.fetch(indexer, &chain, true).await
    }))
    .await;
    let mut releases: Vec<IndexerRelease> = Vec::new();
    let mut read = 0usize;
    for (indexer, outcome) in indexers.iter().zip(outcomes) {
        match outcome {
            Ok(list) => {
                read += 1;
                releases.extend(list);
            }
            Err(e) => {
                tracing::warn!(indexer = %indexer.name, error = %e, "RSS sync could not read an indexer");
                failures.push(IndexerFailure {
                    indexer_id: indexer.id,
                    indexer: indexer.name.clone(),
                    message: search::describe(&e),
                });
            }
        }
    }
    let failed = |failures: &[IndexerFailure]| {
        failures
            .iter()
            .map(|f| format!("{} ({})", f.indexer, f.message))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if read == 0 {
        return Err(format!("No indexer could be read: {}", failed(&failures)));
    }

    let library = kitsunarr_library::decision_input::library_titles(&state.db)
        .await
        .map_err(|e| e.to_string())?;
    let candidates: Vec<decision::Release> = releases
        .iter()
        .map(|r| search::decision_release(r, extras.get(&r.indexer_id)))
        .collect();

    // `GetRssDecision`'s mapping: specials first (they need the series' episodes), then the
    // series named by the title or the release's ids.
    let mut loaded = Loaded::new();
    let mut groups: Vec<(i64, Vec<usize>)> = Vec::new();
    for (i, r) in candidates.iter().enumerate() {
        let mut target = None;
        if let Some(id) = decision::rss_special_candidate(r, &library)
            && load(state, &mut loaded, id)
                .await
                .is_some_and(|inputs| decision::rss_is_special_of(r, &inputs.series))
        {
            target = Some(id);
        }
        let Some(id) = target.or_else(|| decision::rss_series(r, &library)) else {
            continue;
        };
        match groups.iter_mut().find(|(g, _)| *g == id) {
            Some((_, members)) => members.push(i),
            None => groups.push((id, vec![i])),
        }
    }
    let matched: usize = groups.iter().map(|(_, m)| m.len()).sum();

    let criteria = SearchCriteria::rss();
    let mut approved = 0usize;
    let mut grabbed = Vec::new();
    let mut grab_failures = Vec::new();
    let mut unavailable = Vec::new();
    for (series_id, members) in groups {
        let Some(inputs) = load(state, &mut loaded, series_id).await else {
            continue;
        };
        let group: Vec<decision::Release> =
            members.iter().map(|&i| candidates[i].clone()).collect();
        let engine = Engine {
            series: &inputs.series,
            library: &inputs.library,
            formats: &inputs.formats,
            settings: &inputs.settings,
        };
        let decisions = engine.prioritize(
            Engine::dedupe(engine.evaluate(&group, &criteria), &group),
            &group,
        );
        approved += decisions.iter().filter(|d| d.approved()).count();
        if !decisions.iter().any(|d| d.approved()) {
            continue;
        }
        let run = SearchRun {
            decisions,
            releases: members.iter().map(|&i| releases[i].clone()).collect(),
            failures: Vec::new(),
            searched_indexers: 0,
            description: String::new(),
            warnings: Vec::new(),
            series: inputs.series.clone(),
        };
        let (g, f) = search::process_decisions(state, &run, series_id, &mut unavailable).await;
        grabbed.extend(g);
        grab_failures.extend(f);
    }

    let mut summary = format!(
        "{} releases found ({matched} for library series), {approved} approved, {} grabbed",
        releases.len(),
        grabbed.len()
    );
    if !grabbed.is_empty() {
        let titles: Vec<&str> = grabbed.iter().map(|g| g.title.as_str()).collect();
        summary.push_str(&format!(": {}", titles.join(", ")));
    }
    for f in &grab_failures {
        summary.push_str(&format!("; could not grab {}: {}", f.title, f.message));
    }
    if !failures.is_empty() {
        summary.push_str(&format!("; indexers that failed: {}", failed(&failures)));
    }
    tracing::info!(%summary, "RSS sync completed");
    Ok(summary)
}
