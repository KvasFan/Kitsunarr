//! Import list sync: Sonarr's `ImportListSyncService` and `FetchAndParseImportListService`
//! with Tanukarr's `ImportListIdentity` (AniList, MyAnimeList and TMDB identities).
//!
//! Every enabled list with automatic add is read when due (its minimum refresh interval passed
//! and it is not backing off after failures); lists not due reuse the membership stored at
//! their last complete read. Each item is matched with the library by any shared catalogue id;
//! excluded items are skipped, existing titles optionally tagged, and new ones added with the
//! list's root folder, profile, monitoring, type and tags. A failed read keeps the previous
//! membership. Afterwards, tags of "sync" lists are removed from titles no longer on them, and
//! the library is cleaned per the list sync level once every list has synced since an item
//! left a list.

use std::collections::{HashMap, HashSet};

use kitsunarr_db::import_lists::{self as db_lists, ListItemRow};
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_domain::api::library::{SeasonMonitoring, SeriesUpdate};
use kitsunarr_domain::api::metadata::{AddTitleRequest, MonitorOption};
use kitsunarr_domain::api::settings::{ImportListOptionsSettings, ListSyncLevel, ProviderKind};
use kitsunarr_domain::{ExternalId, ExternalIdKind, MetadataProvider, SeriesType};
use kitsunarr_importlists::ListItem;
use serde_json::Value;

use crate::AppState;

/// The scheduled task's name.
pub const TASK: &str = "ImportListSync";

/// A list's behaviour (Sonarr's `ImportListDefinition`), read from the stored options.
#[derive(Debug, Clone)]
pub struct ListOptions {
    pub id: i64,
    pub name: String,
    pub implementation: String,
    pub automatic_add: bool,
    pub search_for_missing: bool,
    pub monitor: MonitorOption,
    pub monitor_none: bool,
    pub root_folder_path: Option<String>,
    pub quality_profile_id: Option<i64>,
    pub series_type: SeriesType,
    pub season_folder: bool,
    pub tag_existing: TagExisting,
    pub preferred: Option<MetadataProvider>,
    pub tags: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagExisting {
    None,
    Add,
    Sync,
}

/// Sonarr stores enums as names (API) or numbers (database); both are read.
fn enum_value<'a>(v: Option<&'a Value>, names: &[&'a str]) -> Option<&'a str> {
    match v? {
        Value::String(s) => names.iter().copied().find(|n| n.eq_ignore_ascii_case(s)),
        Value::Number(n) => names.get(usize::try_from(n.as_u64()?).ok()?).copied(),
        _ => None,
    }
}

impl ListOptions {
    pub fn from_record(p: &ProviderRecord) -> Self {
        let o = &p.options;
        let monitor = enum_value(
            o.get("shouldMonitor"),
            &[
                "unknown",
                "all",
                "future",
                "missing",
                "existing",
                "firstSeason",
                "lastSeason",
                "latestSeason",
                "pilot",
                "recent",
                "monitorSpecials",
                "unmonitorSpecials",
                "none",
                "skip",
            ],
        )
        .unwrap_or("all");
        Self {
            id: p.id,
            name: p.name.clone(),
            implementation: p.implementation.clone(),
            automatic_add: p.enabled
                && o.get("enableAutomaticAdd")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            search_for_missing: o
                .get("searchForMissingEpisodes")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            monitor: match monitor {
                "future" => MonitorOption::Future,
                "firstSeason" => MonitorOption::FirstSeason,
                "lastSeason" | "latestSeason" => MonitorOption::LatestSeason,
                "none" => MonitorOption::None,
                _ => MonitorOption::All,
            },
            monitor_none: monitor == "none",
            root_folder_path: o
                .get("rootFolderPath")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            quality_profile_id: o.get("qualityProfileId").and_then(Value::as_i64),
            series_type: match enum_value(o.get("seriesType"), &["standard", "daily", "anime"]) {
                Some("daily") => SeriesType::Daily,
                Some("anime") => SeriesType::Anime,
                _ => SeriesType::Standard,
            },
            season_folder: o
                .get("seasonFolder")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            tag_existing: match enum_value(o.get("tagExisting"), &["none", "add", "sync"]) {
                Some("add") => TagExisting::Add,
                Some("sync") => TagExisting::Sync,
                _ => TagExisting::None,
            },
            preferred: match enum_value(
                o.get("preferredMetadataSource"),
                &["automatic", "tvdb", "tmdb", "aniList", "myAnimeList"],
            ) {
                Some("tvdb") => Some(MetadataProvider::Tvdb),
                Some("tmdb") => Some(MetadataProvider::TmdbTv),
                Some("aniList") => Some(MetadataProvider::AniList),
                Some("myAnimeList") => Some(MetadataProvider::MyAnimeList),
                _ => None,
            },
            tags: p.tags.clone(),
        }
    }
}

pub fn to_row(list_id: i64, i: &ListItem) -> ListItemRow {
    ListItemRow {
        import_list_id: list_id,
        title: i.title.clone(),
        year: i.year,
        tvdb_id: i.tvdb_id,
        tmdb_id: i.tmdb_id,
        imdb_id: i.imdb_id.clone(),
        mal_id: i.mal_id,
        anilist_id: i.anilist_id,
    }
}

pub fn from_row(r: &ListItemRow) -> ListItem {
    ListItem {
        title: r.title.clone(),
        year: r.year,
        tvdb_id: r.tvdb_id,
        tmdb_id: r.tmdb_id,
        imdb_id: r.imdb_id.clone(),
        mal_id: r.mal_id,
        anilist_id: r.anilist_id,
        seasons: Vec::new(),
    }
}

/// Identity keys of a list item.
fn item_keys(i: &ListItem) -> Vec<String> {
    let mut k = Vec::new();
    if let Some(v) = i.tvdb_id {
        k.push(format!("tvdb:{v}"));
    }
    if let Some(v) = i.tmdb_id {
        k.push(format!("tmdb:{v}"));
    }
    if let Some(v) = &i.imdb_id {
        k.push(format!("imdb:{v}"));
    }
    if let Some(v) = i.anilist_id {
        k.push(format!("aniList:{v}"));
    }
    if let Some(v) = i.mal_id {
        k.push(format!("myAnimeList:{v}"));
    }
    k
}

fn provider_key(p: &str) -> &str {
    match p {
        "tmdb" | "tmdbMovie" => "tmdb",
        other => other,
    }
}

/// A library series as list matching sees it.
#[derive(Debug, Clone)]
pub struct LibrarySeries {
    pub id: i64,
    pub title: String,
    pub provider: String,
    pub metadata_id: String,
    pub monitored: bool,
    pub tags: Vec<i64>,
    pub keys: HashSet<String>,
}

impl LibrarySeries {
    /// `ImportListIdentity.Matches`: an AniList item matches an AniList-owned series only by
    /// its owning id; otherwise any shared catalogue id matches.
    pub fn matches(&self, item: &ListItem) -> bool {
        if let Some(a) = item.anilist_id
            && self.provider == "aniList"
        {
            return self.metadata_id == a.to_string();
        }
        item_keys(item).iter().any(|k| self.keys.contains(k))
    }
}

pub async fn library_series(state: &AppState) -> Result<Vec<LibrarySeries>, String> {
    Ok(db_lists::series_identities(state.db.pool())
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| {
            let mut keys: HashSet<String> = s
                .external
                .iter()
                .map(|(kind, value)| format!("{kind}:{value}"))
                .collect();
            keys.insert(format!("{}:{}", provider_key(&s.provider), s.metadata_id));
            LibrarySeries {
                id: s.id,
                title: s.title,
                provider: s.provider,
                metadata_id: s.metadata_id,
                monitored: s.monitored,
                tags: s.tags,
                keys,
            }
        })
        .collect())
}

/// `ImportListIdentity.GetSource`: the preferred catalogue when the item has its id, else TheTVDB,
/// AniList, TMDB, MyAnimeList, among the catalogues usable here.
fn candidate(
    state: &AppState,
    item: &ListItem,
    preferred: Option<MetadataProvider>,
) -> Option<(MetadataProvider, String)> {
    let id_for = |p: MetadataProvider| -> Option<String> {
        match p {
            MetadataProvider::Tvdb => item.tvdb_id.map(|v| v.to_string()),
            MetadataProvider::AniList => item.anilist_id.map(|v| v.to_string()),
            MetadataProvider::TmdbTv => item.tmdb_id.map(|v| v.to_string()),
            MetadataProvider::MyAnimeList => item.mal_id.map(|v| v.to_string()),
            _ => None,
        }
    };
    preferred
        .into_iter()
        .chain([
            MetadataProvider::Tvdb,
            MetadataProvider::AniList,
            MetadataProvider::TmdbTv,
            MetadataProvider::MyAnimeList,
        ])
        .filter(|p| state.metadata.available(*p))
        .find_map(|p| id_for(p).map(|id| (p, id)))
}

fn exclusion_matches(
    exclusions: &[(String, String)],
    item: &ListItem,
    cand: Option<&(MetadataProvider, String)>,
) -> bool {
    exclusions.iter().any(|(provider, id)| {
        let item_id = match provider.as_str() {
            "tvdb" => item.tvdb_id.map(|v| v.to_string()),
            "tmdb" => item.tmdb_id.map(|v| v.to_string()),
            "aniList" => item.anilist_id.map(|v| v.to_string()),
            "myAnimeList" => item.mal_id.map(|v| v.to_string()),
            _ => None,
        };
        item_id.as_deref() == Some(id.as_str())
            || cand.is_some_and(|(p, cid)| p.as_str() == provider && cid == id)
    })
}

/// What a sync did.
#[derive(Debug, Default)]
pub struct SyncSummary {
    pub lists_read: usize,
    pub lists_failed: Vec<String>,
    pub items: usize,
    pub added: Vec<String>,
    pub tagged: usize,
    pub untagged: usize,
    pub cleaned: usize,
}

impl std::fmt::Display for SyncSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} lists read, {} items, {} titles added",
            self.lists_read,
            self.items,
            self.added.len()
        )?;
        if self.tagged + self.untagged > 0 {
            write!(f, ", {} tagged, {} untagged", self.tagged, self.untagged)?;
        }
        if self.cleaned > 0 {
            write!(f, ", {} cleaned from the library", self.cleaned)?;
        }
        if !self.lists_failed.is_empty() {
            write!(f, "; failed: {}", self.lists_failed.join(", "))?;
        }
        Ok(())
    }
}

async fn lists(state: &AppState) -> Result<Vec<(ProviderRecord, ListOptions)>, String> {
    Ok(providers::list(state.db.pool(), ProviderKind::ImportList)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|p| kitsunarr_importlists::is_supported(&p.implementation))
        .map(|p| {
            let o = ListOptions::from_record(&p);
            (p, o)
        })
        .collect())
}

/// Reads one list now and stores its membership; `Err` keeps the previous membership.
pub async fn read_list(state: &AppState, p: &ProviderRecord) -> Result<Vec<ListItem>, String> {
    let read = async {
        let mut settings = p.settings.clone();
        if let Some(renewed) =
            kitsunarr_importlists::renew(&state.http, &p.implementation, &settings).await?
        {
            // Keep the renewed token: the old refresh token may be single-use.
            let mut rec = p.clone();
            rec.settings = renewed.clone();
            if let Err(e) = providers::update(state.db.pool(), &rec).await {
                tracing::warn!(list = %p.name, error = %e, "the renewed list token could not be stored");
            }
            settings = renewed;
        }
        kitsunarr_importlists::fetch(&state.http, &p.implementation, &settings).await
    };
    match read.await {
        Ok(items) => {
            let rows: Vec<ListItemRow> = items.iter().map(|i| to_row(p.id, i)).collect();
            let removed = db_lists::replace_items(state.db.pool(), p.id, &rows)
                .await
                .map_err(|e| e.to_string())?;
            db_lists::record_sync(state.db.pool(), p.id, removed > 0)
                .await
                .map_err(|e| e.to_string())?;
            Ok(items)
        }
        Err(e) => {
            let retry = match &e {
                kitsunarr_importlists::ListError::RateLimited(d) => Some(
                    d.and_then(|d| chrono::Duration::from_std(d).ok())
                        .unwrap_or(chrono::Duration::hours(1)),
                ),
                _ => None,
            };
            tracing::warn!(list = %p.name, error = %e, "import list could not be read");
            let _ = db_lists::record_failure(state.db.pool(), p.id, &e.to_string(), retry).await;
            Err(e.to_string())
        }
    }
}

/// Syncs every list (or the one `only`), reading lists when due or when `force`.
pub async fn sync(state: &AppState, only: Option<i64>, force: bool) -> Result<String, String> {
    // A scheduled sync is automation; "Sync now" is a manual action.
    if !force && crate::library_tools::paused(state).await {
        return Ok(crate::library_tools::PAUSED.into());
    }
    let all = lists(state).await?;
    let selected: Vec<_> = all
        .iter()
        // A list without automatic add is read when synced on its own (for its preview); its
        // shows are only added through the selective add.
        .filter(|(p, o)| match only {
            Some(id) => id == o.id && (o.automatic_add || p.enabled),
            None => o.automatic_add,
        })
        .collect();
    if selected.is_empty() {
        return Ok("No import lists with automatic add enabled".into());
    }
    let mut summary = SyncSummary::default();
    let now = chrono::Utc::now();
    let mut items: Vec<(ListOptions, ListItem)> = Vec::new();
    for (p, o) in &selected {
        let status = db_lists::status(state.db.pool(), p.id)
            .await
            .map_err(|e| e.to_string())?;
        let interval = chrono::Duration::from_std(kitsunarr_importlists::min_refresh_interval(
            &p.implementation,
        ))
        .unwrap_or_default();
        let due = force
            || (status.disabled_till().is_none_or(|t| t <= now)
                && status.last_info_sync().is_none_or(|l| l + interval <= now));
        if due {
            match read_list(state, p).await {
                Ok(read) => {
                    summary.lists_read += 1;
                    items.extend(read.into_iter().map(|i| (o.clone(), i)));
                }
                Err(_) => summary.lists_failed.push(p.name.clone()),
            }
        } else {
            let stored = db_lists::items(state.db.pool(), Some(p.id))
                .await
                .map_err(|e| e.to_string())?;
            items.extend(stored.iter().map(|r| (o.clone(), from_row(r))));
        }
    }
    if !force && summary.lists_read == 0 {
        return Ok(if summary.lists_failed.is_empty() {
            "No list was due for a sync".into()
        } else {
            summary.to_string()
        });
    }
    summary.items = items.len();
    process_items(state, &items, &mut summary).await?;
    remove_sync_tags(state, &all, &mut summary).await?;
    clean_library(state, &all, &mut summary).await?;
    Ok(summary.to_string())
}

/// `ProcessListItems`.
async fn process_items(
    state: &AppState,
    items: &[(ListOptions, ListItem)],
    summary: &mut SyncSummary,
) -> Result<(), String> {
    let exclusions: Vec<(String, String)> = kitsunarr_db::rules::list_exclusions(state.db.pool())
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|e| (e.provider, e.external_id))
        .collect();
    let library = library_series(state).await?;
    let mut current_tags: HashMap<i64, Vec<i64>> =
        library.iter().map(|s| (s.id, s.tags.clone())).collect();
    let mut tags_to_add: HashMap<i64, HashSet<i64>> = HashMap::new();
    // Titles added in this run (their identity keys) and their tags, by series id.
    let mut added: Vec<(HashSet<String>, i64)> = Vec::new();
    for (list, item) in items {
        if !list.automatic_add {
            continue;
        }
        let mut cand = candidate(state, item, list.preferred);
        if cand.is_none()
            && let Some(imdb) = &item.imdb_id
        {
            // No catalogue id: look the IMDb id up on TheTVDB (Sonarr's SearchForNewSeriesByImdbId).
            if let Ok(found) = state.metadata.search(MetadataProvider::Tvdb, imdb).await {
                cand = found
                    .first()
                    .map(|r| (MetadataProvider::Tvdb, r.id.clone()));
            }
        }
        if exclusion_matches(&exclusions, item, cand.as_ref()) {
            tracing::debug!(title = %item.title, "import list item excluded");
            continue;
        }
        let cand_key = cand
            .as_ref()
            .map(|(p, id)| format!("{}:{id}", provider_key(p.as_str())));
        if let Some(existing) = library
            .iter()
            .find(|s| s.matches(item) || cand_key.as_ref().is_some_and(|k| s.keys.contains(k)))
        {
            if list.tag_existing != TagExisting::None && !list.tags.is_empty() {
                tags_to_add
                    .entry(existing.id)
                    .or_default()
                    .extend(list.tags.iter().copied());
            }
            continue;
        }
        let keys: HashSet<String> = item_keys(item).into_iter().chain(cand_key).collect();
        if let Some((_, sid)) = added.iter().find(|(k, _)| !k.is_disjoint(&keys)) {
            // Already added from another list in this run: it gets this list's tags too.
            tags_to_add
                .entry(*sid)
                .or_default()
                .extend(list.tags.iter().copied());
            continue;
        }
        let Some((provider, id)) = cand else {
            tracing::debug!(title = %item.title, "import list item has no usable catalogue id");
            continue;
        };
        let (Some(root), Some(profile)) = (&list.root_folder_path, list.quality_profile_id) else {
            tracing::warn!(list = %list.name, "import list has no root folder or quality profile");
            continue;
        };
        let external: Vec<ExternalId> = [
            (ExternalIdKind::Tvdb, item.tvdb_id.map(|v| v.to_string())),
            (ExternalIdKind::Tmdb, item.tmdb_id.map(|v| v.to_string())),
            (ExternalIdKind::Imdb, item.imdb_id.clone()),
            (
                ExternalIdKind::AniList,
                item.anilist_id.map(|v| v.to_string()),
            ),
            (
                ExternalIdKind::MyAnimeList,
                item.mal_id.map(|v| v.to_string()),
            ),
        ]
        .into_iter()
        .filter_map(|(kind, v)| v.map(|value| ExternalId { kind, value }))
        .collect();
        if let Ok(Some(found)) =
            kitsunarr_library::find_existing(&state.db, provider, &id, &external).await
        {
            if found.media_type == "series"
                && list.tag_existing != TagExisting::None
                && !list.tags.is_empty()
            {
                tags_to_add
                    .entry(found.id)
                    .or_default()
                    .extend(list.tags.iter().copied());
            }
            continue;
        }
        let req = AddTitleRequest {
            provider,
            id: id.clone(),
            root_folder_path: root.clone(),
            quality_profile_id: profile,
            monitor: if item.seasons.is_empty() {
                list.monitor
            } else {
                MonitorOption::None
            },
            series_type: Some(
                if matches!(
                    provider,
                    MetadataProvider::AniList | MetadataProvider::MyAnimeList
                ) {
                    SeriesType::Anime
                } else {
                    list.series_type
                },
            ),
            season_folder: list.season_folder,
            tags: list.tags.clone(),
            search_for_missing: list.search_for_missing,
        };
        match kitsunarr_library::add_title(&state.db, &state.metadata, &req).await {
            Ok(r) => {
                summary.added.push(item.title.clone());
                state.events.publish(ServerEvent::LibraryChanged {
                    media_type: r.media_type.clone(),
                    ids: vec![r.id],
                    deleted: false,
                });
                if r.media_type != "series" {
                    continue;
                }
                added.push((keys, r.id));
                current_tags.insert(r.id, list.tags.clone());
                if !item.seasons.is_empty() {
                    let update = SeriesUpdate {
                        monitored: Some(!list.monitor_none),
                        seasons: Some(
                            item.seasons
                                .iter()
                                .map(|(n, m)| SeasonMonitoring {
                                    season_number: *n as i32,
                                    monitored: *m,
                                })
                                .collect(),
                        ),
                        ..Default::default()
                    };
                    if let Err(e) =
                        kitsunarr_db::library::series::apply_update(&state.db, r.id, &update).await
                    {
                        tracing::warn!(error = %e, "could not apply the list's season monitoring");
                    }
                }
                if let Some(series) = kitsunarr_library::notify::series_ref(&state.db, r.id).await {
                    kitsunarr_library::notify::spawn(
                        &state.db,
                        &state.downloads.notifier,
                        kitsunarr_notifications::Event::SeriesAdd(Box::new(series)),
                    );
                }
                if req.search_for_missing {
                    crate::routes::wanted::search_missing_of_series(state, r.id);
                }
            }
            Err(e) => {
                tracing::warn!(title = %item.title, error = %e, "import list title could not be added");
            }
        }
    }
    for (sid, tags) in tags_to_add {
        let current = current_tags.get(&sid).cloned().unwrap_or_default();
        let mut next: Vec<i64> = current.clone();
        for t in tags {
            if !next.contains(&t) {
                next.push(t);
            }
        }
        if next.len() != current.len() {
            set_tags(state, sid, next).await?;
            summary.tagged += 1;
        }
    }
    Ok(())
}

async fn set_tags(state: &AppState, series_id: i64, tags: Vec<i64>) -> Result<(), String> {
    kitsunarr_db::library::series::apply_update(
        &state.db,
        series_id,
        &SeriesUpdate {
            tags: Some(tags),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    state.events.publish(ServerEvent::LibraryChanged {
        media_type: "series".into(),
        ids: vec![series_id],
        deleted: false,
    });
    Ok(())
}

fn synced_ok(status: &[db_lists::ListStatus], list_id: i64) -> bool {
    status
        .iter()
        .find(|s| s.import_list_id == list_id)
        .is_some_and(|s| s.disabled_till.is_none() && s.last_info_sync.is_some())
}

/// `RemoveTagsFromSeriesNotOnLists`: tags of "sync" lists leave titles none of the lists with
/// that tag contain, once each of those lists has synced successfully.
async fn remove_sync_tags(
    state: &AppState,
    all: &[(ProviderRecord, ListOptions)],
    summary: &mut SyncSummary,
) -> Result<(), String> {
    let lists: Vec<&ListOptions> = all
        .iter()
        .map(|(_, o)| o)
        .filter(|o| o.automatic_add)
        .collect();
    let status = db_lists::all_status(state.db.pool())
        .await
        .map_err(|e| e.to_string())?;
    let mut sync_tags: HashSet<i64> = lists
        .iter()
        .filter(|l| l.tag_existing == TagExisting::Sync)
        .flat_map(|l| l.tags.iter().copied())
        .collect();
    let lists_by_tag: HashMap<i64, Vec<i64>> = sync_tags
        .iter()
        .map(|t| {
            (
                *t,
                lists
                    .iter()
                    .filter(|l| l.tags.contains(t))
                    .map(|l| l.id)
                    .collect(),
            )
        })
        .collect();
    sync_tags.retain(|t| lists_by_tag[t].iter().all(|id| synced_ok(&status, *id)));
    if sync_tags.is_empty() {
        return Ok(());
    }
    let rows = db_lists::items(state.db.pool(), None)
        .await
        .map_err(|e| e.to_string())?;
    let mut by_list: HashMap<i64, Vec<ListItem>> = HashMap::new();
    for r in &rows {
        by_list
            .entry(r.import_list_id)
            .or_default()
            .push(from_row(r));
    }
    for s in library_series(state).await? {
        let remove: Vec<i64> = s
            .tags
            .iter()
            .copied()
            .filter(|t| {
                sync_tags.contains(t)
                    && !lists_by_tag[t].iter().any(|l| {
                        by_list
                            .get(l)
                            .is_some_and(|items| items.iter().any(|i| s.matches(i)))
                    })
            })
            .collect();
        if remove.is_empty() {
            continue;
        }
        let next = s
            .tags
            .iter()
            .copied()
            .filter(|t| !remove.contains(t))
            .collect();
        set_tags(state, s.id, next).await?;
        summary.untagged += 1;
    }
    Ok(())
}

/// `TryCleanLibrary`: runs when every automatic-add list has synced successfully and some list
/// lost an item since the last clean.
async fn clean_library(
    state: &AppState,
    all: &[(ProviderRecord, ListOptions)],
    summary: &mut SyncSummary,
) -> Result<(), String> {
    let options: ImportListOptionsSettings =
        kv::get_or_default(state.db.pool(), kv::KEY_IMPORT_LIST_OPTIONS)
            .await
            .map_err(|e| e.to_string())?;
    if options.list_sync_level == ListSyncLevel::Disabled {
        return Ok(());
    }
    let status = db_lists::all_status(state.db.pool())
        .await
        .map_err(|e| e.to_string())?;
    let lists: Vec<&ListOptions> = all
        .iter()
        .map(|(_, o)| o)
        .filter(|o| o.automatic_add)
        .collect();
    if lists.is_empty() || !lists.iter().all(|l| synced_ok(&status, l.id)) {
        return Ok(());
    }
    let pending = lists.iter().any(|l| {
        status
            .iter()
            .any(|s| s.import_list_id == l.id && s.has_removed_item_since_last_clean)
    });
    if !pending {
        return Ok(());
    }
    let items: Vec<ListItem> = db_lists::items(state.db.pool(), None)
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(from_row)
        .collect();
    for s in library_series(state).await? {
        if items.iter().any(|i| s.matches(i)) {
            continue;
        }
        match options.list_sync_level {
            ListSyncLevel::LogOnly => {
                tracing::info!(series = %s.title, "in the library but on no import list; you might want to unmonitor or remove it");
            }
            ListSyncLevel::KeepAndUnmonitor if s.monitored => {
                tracing::info!(series = %s.title, "on no import list: unmonitored");
                kitsunarr_db::library::series::apply_update(
                    &state.db,
                    s.id,
                    &SeriesUpdate {
                        monitored: Some(false),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                summary.cleaned += 1;
            }
            ListSyncLevel::KeepAndTag => {
                if let Some(tag) = options.list_sync_tag
                    && !s.tags.contains(&tag)
                {
                    tracing::info!(series = %s.title, "on no import list: tagged");
                    let mut next = s.tags.clone();
                    next.push(tag);
                    set_tags(state, s.id, next).await?;
                    summary.cleaned += 1;
                }
            }
            _ => {}
        }
    }
    db_lists::mark_cleaned(state.db.pool())
        .await
        .map_err(|e| e.to_string())
}

/// Tanukarr's selective add from a list preview: the chosen shows of the list's stored
/// membership are added with the given root folder and profile (monitored or not, no search).
/// Titles in the library or excluded are skipped. Unlike Tanukarr, the list's preferred
/// catalogue is honoured.
pub async fn add_selected(
    state: &AppState,
    list_id: i64,
    req: &kitsunarr_domain::api::import_lists::ImportListAddRequest,
) -> Result<Vec<kitsunarr_domain::api::import_lists::ImportListAddOutcome>, crate::error::ApiError>
{
    use crate::error::ApiError;
    use kitsunarr_domain::api::import_lists::ImportListAddOutcome;
    if req.items.is_empty() || req.items.len() > 25 {
        return Err(ApiError::field("items", "Choose 1 to 25 shows."));
    }
    let record = providers::get(state.db.pool(), ProviderKind::ImportList, list_id).await?;
    let list = ListOptions::from_record(&record);
    let rows = db_lists::items(state.db.pool(), Some(list_id)).await?;
    if req.items.iter().any(|i| *i >= rows.len()) {
        return Err(ApiError::field(
            "items",
            "A chosen show is not in the list any more.",
        ));
    }
    let exclusions: Vec<(String, String)> = kitsunarr_db::rules::list_exclusions(state.db.pool())
        .await?
        .into_iter()
        .map(|e| (e.provider, e.external_id))
        .collect();
    let mut out = Vec::new();
    for &i in &req.items {
        let item = from_row(&rows[i]);
        let outcome =
            |o: &str, message: Option<String>, series_id: Option<i64>| ImportListAddOutcome {
                title: item.title.clone(),
                outcome: o.into(),
                message,
                series_id,
            };
        let library = library_series(state).await.map_err(ApiError::internal)?;
        if let Some(s) = library.iter().find(|s| s.matches(&item)) {
            out.push(outcome("exists", None, Some(s.id)));
            continue;
        }
        let mut cand = candidate(state, &item, list.preferred);
        if cand.is_none()
            && let Some(imdb) = &item.imdb_id
            && let Ok(found) = state.metadata.search(MetadataProvider::Tvdb, imdb).await
        {
            cand = found
                .first()
                .map(|r| (MetadataProvider::Tvdb, r.id.clone()));
        }
        if exclusion_matches(&exclusions, &item, cand.as_ref()) {
            out.push(outcome("excluded", None, None));
            continue;
        }
        let Some((provider, id)) = cand else {
            out.push(outcome(
                "unsupported",
                Some("No usable catalogue id.".into()),
                None,
            ));
            continue;
        };
        let add = AddTitleRequest {
            provider,
            id,
            root_folder_path: req.root_folder_path.clone(),
            quality_profile_id: req.quality_profile_id,
            monitor: if req.monitored {
                MonitorOption::All
            } else {
                MonitorOption::None
            },
            series_type: matches!(
                provider,
                MetadataProvider::AniList | MetadataProvider::MyAnimeList
            )
            .then_some(SeriesType::Anime),
            season_folder: true,
            tags: list.tags.clone(),
            search_for_missing: false,
        };
        match crate::routes::metadata::add(axum::extract::State(state.clone()), axum::Json(add))
            .await
        {
            Ok((_, axum::Json(r))) => out.push(outcome("added", None, Some(r.id))),
            Err(e) => out.push(outcome("failed", Some(e.message()), None)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_read_names_and_numbers() {
        let mut p = ProviderRecord {
            id: 1,
            kind: ProviderKind::ImportList,
            name: "a".into(),
            implementation: "AniListImport".into(),
            protocol: kitsunarr_domain::DownloadProtocol::Unknown,
            enabled: true,
            priority: 0,
            tags: vec![3],
            options: serde_json::from_value(serde_json::json!({
                "shouldMonitor": "firstSeason", "seriesType": 2, "tagExisting": "sync",
                "preferredMetadataSource": 3, "rootFolderPath": "/tv", "qualityProfileId": 4
            }))
            .unwrap(),
            settings: Default::default(),
        };
        let o = ListOptions::from_record(&p);
        assert_eq!(o.monitor, MonitorOption::FirstSeason);
        assert_eq!(o.series_type, SeriesType::Anime);
        assert_eq!(o.tag_existing, TagExisting::Sync);
        assert_eq!(o.preferred, Some(MetadataProvider::AniList));
        assert!(o.automatic_add && o.search_for_missing);
        p.options
            .insert("shouldMonitor".into(), serde_json::json!(12));
        p.options
            .insert("enableAutomaticAdd".into(), serde_json::json!(false));
        let o = ListOptions::from_record(&p);
        assert!(o.monitor_none && !o.automatic_add);
    }

    #[test]
    fn anilist_items_match_anilist_series_by_owning_id_only() {
        let s = LibrarySeries {
            id: 1,
            title: "Frieren".into(),
            provider: "aniList".into(),
            metadata_id: "154587".into(),
            monitored: true,
            tags: vec![],
            keys: ["aniList:154587", "myAnimeList:52991", "tvdb:424536"]
                .map(String::from)
                .into_iter()
                .collect(),
        };
        let item = |a: Option<i64>, t: Option<i64>| ListItem {
            anilist_id: a,
            tvdb_id: t,
            ..Default::default()
        };
        assert!(s.matches(&item(Some(154587), None)));
        // Season 2 shares the TVDB id but is a different AniList entry.
        assert!(!s.matches(&item(Some(182255), Some(424536))));
        assert!(s.matches(&item(None, Some(424536))));
    }
}
