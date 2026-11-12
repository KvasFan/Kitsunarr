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

/// A pending release as stored (Sonarr's `PendingRelease.Release`).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredRelease {
    indexer_id: i64,
    indexer: String,
    guid: String,
    title: String,
    size: i64,
    download_url: Option<String>,
    info_url: Option<String>,
    comment_url: Option<String>,
    publish_date: chrono::DateTime<chrono::Utc>,
    protocol: kitsunarr_domain::DownloadProtocol,
    tvdb_id: i64,
    tv_rage_id: i64,
    imdb_id: Option<String>,
    languages: Vec<i32>,
    indexer_flags: u32,
    info_hash: Option<String>,
    magnet_url: Option<String>,
    seeders: Option<i64>,
    peers: Option<i64>,
}

impl StoredRelease {
    fn of(r: &IndexerRelease) -> Self {
        let x = &r.release;
        Self {
            indexer_id: r.indexer_id,
            indexer: r.indexer.clone(),
            guid: x.guid.clone(),
            title: x.title.clone(),
            size: x.size,
            download_url: x.download_url.clone(),
            info_url: x.info_url.clone(),
            comment_url: x.comment_url.clone(),
            publish_date: x.publish_date,
            protocol: x.protocol,
            tvdb_id: x.tvdb_id,
            tv_rage_id: x.tv_rage_id,
            imdb_id: x.imdb_id.clone(),
            languages: x.languages.iter().map(|l| l.id).collect(),
            indexer_flags: x.indexer_flags,
            info_hash: x.info_hash.clone(),
            magnet_url: x.magnet_url.clone(),
            seeders: x.seeders,
            peers: x.peers,
        }
    }

    pub(crate) fn release(self) -> IndexerRelease {
        IndexerRelease {
            indexer_id: self.indexer_id,
            indexer: self.indexer,
            release: kitsunarr_indexers::Release {
                guid: self.guid,
                title: self.title,
                size: self.size,
                download_url: self.download_url,
                info_url: self.info_url,
                comment_url: self.comment_url,
                publish_date: self.publish_date,
                protocol: self.protocol,
                tvdb_id: self.tvdb_id,
                tv_rage_id: self.tv_rage_id,
                imdb_id: self.imdb_id,
                languages: self
                    .languages
                    .iter()
                    .filter_map(|id| kitsunarr_domain::language::Language::by_id(*id))
                    .collect(),
                indexer_flags: self.indexer_flags,
                info_hash: self.info_hash,
                magnet_url: self.magnet_url,
                seeders: self.seeders,
                peers: self.peers,
            },
        }
    }
}

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

/// What became of one release.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReleaseOutcome {
    pub series_id: Option<i64>,
    pub approved: bool,
    /// Held back only by a delay profile (pending).
    pub temporarily_rejected: bool,
    pub rejections: Vec<String>,
    pub grabbed: bool,
}

pub(crate) struct Processed {
    pub matched: usize,
    pub approved: usize,
    pub grabbed: Vec<kitsunarr_domain::api::search::AutomaticGrab>,
    pub grab_failures: Vec<kitsunarr_domain::api::search::AutomaticGrabFailure>,
    /// One per release, in order.
    pub outcomes: Vec<ReleaseOutcome>,
}

/// `GetRssDecision` and `ProcessDecisions`: maps releases to library series, decides with the
/// RSS rules, keeps the ones a delay holds back as pending and grabs the approved ones best
/// first. RSS sync and pushed releases (`/api/v3/release/push`) both go through it.
pub(crate) async fn process(
    state: &AppState,
    releases: &[IndexerRelease],
    extras: &search::Extras,
) -> Result<Processed, String> {
    let library = kitsunarr_library::decision_input::library_titles(&state.db)
        .await
        .map_err(|e| e.to_string())?;
    let mut outcomes: Vec<ReleaseOutcome> = releases
        .iter()
        .map(|_| ReleaseOutcome {
            rejections: vec!["Unknown Series".into()],
            ..Default::default()
        })
        .collect();
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
        for d in &decisions {
            outcomes[members[d.release]] = ReleaseOutcome {
                series_id: Some(series_id),
                approved: d.approved(),
                temporarily_rejected: !d.rejections.is_empty()
                    && d.rejections
                        .iter()
                        .all(|x| x.reason == decision::RejectionReason::MinimumAgeDelay),
                rejections: d.rejections.iter().map(|x| x.message.clone()).collect(),
                grabbed: false,
            };
        }
        // Held back only by the delay profile: pending; anything else: no longer pending.
        for d in &decisions {
            let r = &releases[members[d.release]];
            let delayed = !d.rejections.is_empty()
                && !d.episodes.is_empty()
                && d.rejections
                    .iter()
                    .all(|x| x.reason == decision::RejectionReason::MinimumAgeDelay);
            let db = state.db.pool();
            let result = if delayed {
                let episodes: Vec<i64> = d.episodes.iter().map(|e| e.id).collect();
                kitsunarr_db::pending::upsert(
                    db,
                    series_id,
                    r.indexer_id,
                    &r.release.guid,
                    &r.release.title,
                    &serde_json::to_string(&StoredRelease::of(r)).unwrap_or_default(),
                    &episodes,
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
        let (g, f) =
            search::process_decisions(state, &run, series_id, &mut unavailable, "Rss").await;
        // A grab for an episode ends every pending release of that episode (and the grabbed one).
        for grab in &g {
            if let Some(d) = run
                .decisions
                .iter()
                .find(|d| d.approved() && run.releases[d.release].release.title == grab.title)
            {
                outcomes[members[d.release]].grabbed = true;
                let r = &run.releases[d.release];
                let episodes: Vec<i64> = d.episodes.iter().map(|e| e.id).collect();
                let _ =
                    kitsunarr_db::pending::remove(state.db.pool(), r.indexer_id, &r.release.guid)
                        .await;
                let _ =
                    kitsunarr_db::pending::remove_for_episodes(state.db.pool(), &episodes).await;
            }
        }
        grabbed.extend(g);
        grab_failures.extend(f);
    }

    Ok(Processed {
        matched,
        approved,
        grabbed,
        grab_failures,
        outcomes,
    })
}

/// One RSS sync. Returns the summary for the task history; fails when no indexer could be read.
pub async fn sync(state: &AppState) -> Result<String, String> {
    if crate::library_tools::paused(state).await {
        return Ok(crate::library_tools::PAUSED.into());
    }
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

    // Sonarr's pending releases: those a delay profile held back are decided again.
    let pending = kitsunarr_db::pending::list(state.db.pool())
        .await
        .map_err(|e| e.to_string())?;
    let pending_count = pending.len();
    for p in pending {
        if releases
            .iter()
            .any(|r| r.indexer_id == p.indexer_id && r.release.guid == p.guid)
        {
            continue;
        }
        if let Ok(stored) = serde_json::from_str::<StoredRelease>(&p.release) {
            releases.push(stored.release());
        }
    }

    let p = process(state, &releases, &extras).await?;
    let (matched, approved, grabbed, grab_failures) =
        (p.matched, p.approved, p.grabbed, p.grab_failures);

    let mut summary = format!(
        "{} releases found ({matched} for library series{}), {approved} approved, {} grabbed",
        releases.len(),
        if pending_count > 0 {
            format!(", {pending_count} pending before")
        } else {
            String::new()
        },
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
