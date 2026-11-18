//! Converts a TheTVDB series that is really an anime into Tanukarr-style AniList group members.
//!
//! Sonarr keeps an anime franchise as one TheTVDB series with numbered seasons; Tanukarr adds
//! each AniList entry as its own series (season 1, episodes 1..N) inside an anime group, in
//! `{root}/{group folder}/{member folder}`. The conversion walks the franchise's AniList
//! season chain, maps the series' regular episodes onto the entries in order (each entry
//! takes as many episodes as AniList lists for it, the first episode of each chunk checked
//! against the entry's premiere date), and creates only the members the user follows: the
//! entries with files or monitored episodes. The series' folder becomes the group folder and
//! its files move into the members' folders.
//!
//! The plan is computed without writing anything; applying it re-plans, writes everything in
//! one transaction, then moves the files (renames on one file system). A failed move puts the
//! moved files back and rolls the transaction back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDate;
use kitsunarr_db::library::series;
use kitsunarr_db::{Database, DbError, settings};
use kitsunarr_domain::api::library::{
    AnimeConversionMember, AnimeConversionMove, AnimeConversionPlan,
};
use kitsunarr_domain::api::settings::NamingSettings;
use kitsunarr_domain::{MediaKind, MetadataProvider, MonitorNewItems, SeriesType};
use kitsunarr_metadata::{AnimeNode, EpisodeInfo, Metadata, MetadataResult, SeriesInfo};
use sqlx::SqliteConnection;

use crate::grouping::{self, Group, MemberKind, Resolution};
use crate::{LibraryError, LibraryResult};

/// How far apart a chunk's first TheTVDB air date and its AniList entry's premiere may be.
const DATE_TOLERANCE_DAYS: i64 = 21;
/// Episodes listed in a reason before "…".
const LISTED: usize = 5;

/// A regular or special episode of the TheTVDB series.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SourceEpisode {
    pub id: i64,
    pub season: i32,
    pub episode: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub air_date_utc: Option<String>,
    pub monitored: bool,
    pub file_id: Option<i64>,
}

impl SourceEpisode {
    fn label(&self) -> String {
        format!("S{:02}E{:02}", self.season, self.episode)
    }
}

/// An AniList season entry the episodes can map to.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Entry {
    pub id: i64,
    pub title: String,
    pub ordinal: i32,
    /// AniList's episode count; `None` when AniList does not know it yet.
    pub count: Option<usize>,
    /// The premiere (`YYYY-MM-DD`).
    pub first_air: Option<String>,
}

/// The episodes (indices into the regular episodes) one entry takes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Chunk {
    pub entry: usize,
    pub episodes: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Mapping {
    pub chunks: Vec<Chunk>,
    /// Regular episodes beyond the entries (never with files).
    pub leftover: Vec<usize>,
    /// Season-aligned entries whose premiere differs from TheTVDB's air date.
    pub warnings: Vec<String>,
}

fn date(raw: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw?.get(..10)?, "%Y-%m-%d").ok()
}

fn range(episodes: &[&SourceEpisode]) -> String {
    match (episodes.first(), episodes.last()) {
        (Some(a), Some(b)) if a.id == b.id => a.label(),
        (Some(a), Some(b)) => format!("{}–{}", a.label(), b.label()),
        _ => String::new(),
    }
}

fn listed(labels: Vec<String>) -> String {
    let more = labels.len() > LISTED;
    let mut shown: Vec<String> = labels.into_iter().take(LISTED).collect();
    if more {
        shown.push("…".into());
    }
    shown.join(", ")
}

/// Maps the regular episodes (ordered by season and episode) onto the entries in order, each
/// entry taking its episode count. An entry whose count is unknown takes the rest, which only
/// the franchise's last entry may do (`more_in_chain` says whether entries follow the last one
/// given). Each chunk's first episode must have aired within [`DATE_TOLERANCE_DAYS`] of the
/// entry's premiere when both dates are known, unless it opens a TheTVDB season (a streaming
/// premiere weeks before the broadcast: only a warning); episodes left over must have no file.
pub(crate) fn map_episodes(
    regular: &[SourceEpisode],
    entries: &[Entry],
    more_in_chain: bool,
) -> Result<Mapping, String> {
    let mut chunks = Vec::new();
    let mut warnings = Vec::new();
    // The gap between TheTVDB's and AniList's dates at the start of the current TheTVDB
    // season: a streaming release in batches stays that far ahead for the whole season.
    let mut season_gap: Option<(i32, i64)> = None;
    let mut next = 0usize;
    for (i, entry) in entries.iter().enumerate() {
        if next >= regular.len() {
            break;
        }
        let take = match entry.count {
            Some(n) => n,
            None if i + 1 == entries.len() && !more_in_chain => regular.len() - next,
            None => {
                return Err(format!(
                    "AniList does not know how many episodes {} has, and later seasons follow it.",
                    entry.title
                ));
            }
        };
        let end = (next + take).min(regular.len());
        let first = &regular[next];
        let opens_season = next == 0 || regular[next - 1].season != first.season;
        if let (Some(tvdb), Some(anilist)) = (
            date(first.air_date.as_deref()),
            date(entry.first_air.as_deref()),
        ) {
            let gap = (tvdb - anilist).num_days();
            let expected = match season_gap {
                Some((season, g)) if season == first.season && !opens_season => g,
                _ => 0,
            };
            if opens_season {
                season_gap = Some((first.season, gap));
                if gap.abs() > DATE_TOLERANCE_DAYS {
                    warnings.push(format!(
                        "{} aired on {tvdb} but {} (AniList {}) premiered on {anilist}; the entry starts with TheTVDB's season {}, so the seasons are matched as they are.",
                        first.label(),
                        entry.title,
                        entry.id,
                        first.season
                    ));
                }
            } else if (gap - expected).abs() > DATE_TOLERANCE_DAYS {
                return Err(format!(
                    "{} aired on {tvdb} but {} (AniList {}) premiered on {anilist}: the episodes do not line up with the AniList entries.",
                    first.label(),
                    entry.title,
                    entry.id
                ));
            }
        }
        chunks.push(Chunk {
            entry: i,
            episodes: (next..end).collect(),
        });
        next = end;
    }
    let leftover: Vec<usize> = (next..regular.len()).collect();
    let with_files: Vec<String> = leftover
        .iter()
        .map(|i| &regular[*i])
        .filter(|e| e.file_id.is_some())
        .map(SourceEpisode::label)
        .collect();
    if !with_files.is_empty() {
        return Err(format!(
            "Episodes with files go beyond the AniList seasons found ({}).",
            listed(with_files)
        ));
    }
    Ok(Mapping {
        chunks,
        leftover,
        warnings,
    })
}

/// Whether the user follows a chunk: it has a file or a monitored episode.
fn followed(regular: &[SourceEpisode], chunk: &Chunk) -> bool {
    chunk
        .episodes
        .iter()
        .any(|i| regular[*i].file_id.is_some() || regular[*i].monitored)
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Source {
    id: i64,
    title: String,
    path: String,
    root_folder_path: String,
    metadata_provider: String,
    monitored: bool,
    monitor_new_items: String,
    quality_profile_id: i64,
    keep_versions: bool,
    renaming_mode: String,
    season_folder: bool,
}

#[derive(Debug, Clone)]
struct PreparedMember {
    entry: Entry,
    info: SeriesInfo,
    /// Indices into the regular episodes; member episode `k + 1` is `source[k]`.
    source: Vec<usize>,
    /// A library series already owning the entry: (id, path).
    existing: Option<(i64, String)>,
    path: String,
}

#[derive(Debug, Clone)]
struct FilePlan {
    id: i64,
    member: usize,
    relative_path: String,
    /// Relative to the member's folder.
    new_relative: String,
    episodes: Vec<i64>,
}

/// A record linked to an episode file that follows it (an extra or metadata file).
#[derive(Debug, Clone)]
struct Follower {
    id: i64,
    member: usize,
    new_relative: String,
}

#[derive(Debug, Clone)]
struct Version {
    id: i64,
    member: usize,
    relative_path: String,
    archive_path: String,
    episode_ids: Vec<i64>,
}

struct Prepared {
    plan: AnimeConversionPlan,
    source: Option<Source>,
    group: Option<Group>,
    regular: Vec<SourceEpisode>,
    members: Vec<PreparedMember>,
    files: Vec<FilePlan>,
    moves: Vec<(PathBuf, PathBuf)>,
    extras: Vec<Follower>,
    metadata_files: Vec<Follower>,
    versions: Vec<Version>,
    /// Old episode id → (member, member episode number).
    episode_member: HashMap<i64, (usize, i32)>,
    file_count: i64,
}

impl Prepared {
    fn new(id: i64) -> Self {
        Prepared {
            plan: AnimeConversionPlan {
                series_id: id,
                ..Default::default()
            },
            source: None,
            group: None,
            regular: Vec::new(),
            members: Vec::new(),
            files: Vec::new(),
            moves: Vec::new(),
            extras: Vec::new(),
            metadata_files: Vec::new(),
            versions: Vec::new(),
            episode_member: HashMap::new(),
            file_count: 0,
        }
    }
    fn problem(mut self, message: impl Into<String>) -> Self {
        self.plan.problems.push(message.into());
        self.plan.convertible = false;
        self
    }
}

async fn cached(
    metadata: &Metadata,
    cache: &Mutex<HashMap<i64, AnimeNode>>,
    id: i64,
) -> MetadataResult<AnimeNode> {
    if let Some(hit) = cache.lock().expect("relation cache").get(&id).cloned() {
        return Ok(hit);
    }
    let node = metadata.anime_relations(id).await?;
    cache
        .lock()
        .expect("relation cache")
        .insert(id, node.clone());
    Ok(node)
}

/// The conversion of a series: planned only (`dry_run`) or carried out.
pub async fn convert(
    db: &Database,
    metadata: &Metadata,
    series_id: i64,
    dry_run: bool,
) -> LibraryResult<AnimeConversionPlan> {
    if dry_run {
        Ok(prepare(db, metadata, series_id).await?.plan)
    } else {
        apply(db, metadata, series_id).await
    }
}

async fn prepare(db: &Database, metadata: &Metadata, id: i64) -> LibraryResult<Prepared> {
    let mut p = Prepared::new(id);
    let source: Source = sqlx::query_as(
        "SELECT id, title, path, root_folder_path, metadata_provider, monitored, monitor_new_items, \
         quality_profile_id, keep_versions, renaming_mode, season_folder FROM series WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await?
    .ok_or(LibraryError::Db(DbError::NotFound))?;
    p.plan.title = source.title.clone();
    p.plan.group_path = source.path.clone();
    p.plan.convertible = true;
    if source.metadata_provider != MetadataProvider::Tvdb.as_str() {
        return Ok(
            p.problem("Only series from TheTVDB can be converted into AniList group members.")
        );
    }
    let mut ids: Vec<i64> = sqlx::query_scalar::<_, String>(
        "SELECT external_id FROM series_external_ids WHERE series_id = ? AND kind = 'aniList'",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?
    .iter()
    .filter_map(|v| v.trim().parse::<i64>().ok())
    .filter(|v| *v > 0)
    .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(p.problem("The series has no AniList ID."));
    }
    p.source = Some(source.clone());

    // The franchise: the first of the series' AniList IDs that resolves to a season.
    let cache = Mutex::new(HashMap::new());
    let cache = &cache;
    let fetch = move |n: i64| cached(metadata, cache, n);
    let mut resolved = None;
    let mut reasons = Vec::new();
    // Standalone TV entries (no prequel or sequel): converted into one ungrouped AniList
    // series when no franchise resolves, as Tanukarr keeps a standalone title.
    let mut standalone_ids = Vec::new();
    for anime in &ids {
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            grouping::resolve(*anime, &fetch),
        )
        .await;
        match outcome {
            Ok(Ok(Resolution::Resolved(g))) if g.member_kind == MemberKind::Season => {
                resolved = Some((*anime, g));
                break;
            }
            Ok(Ok(Resolution::Resolved(_) | Resolution::Unsupported)) => {}
            Ok(Ok(Resolution::Unrelated)) => standalone_ids.push(*anime),
            Ok(Ok(Resolution::Ambiguous(why))) => reasons.push(format!(
                "the relations of AniList {anime} are ambiguous ({why})"
            )),
            Ok(Ok(Resolution::LimitReached)) => reasons.push(format!(
                "the franchise of AniList {anime} is too large to read"
            )),
            Ok(Err(e)) => {
                return Ok(p.problem(format!("AniList could not be read: {e}")));
            }
            Err(_) => return Ok(p.problem("AniList did not answer in time; try again later.")),
        }
    }
    let (group, chain, start) = match resolved {
        Some((resolved_id, group)) => {
            p.plan.group_id = Some(group.id.clone());
            p.plan.group_title = Some(group.title.clone());
            p.group = Some(group.clone());
            let chain = match grouping::season_chain(group.root, &fetch).await {
                Ok(c) => c,
                Err(e) => return Ok(p.problem(format!("AniList could not be read: {e}"))),
            };
            if let Some(why) = &chain.stopped {
                p.plan.warnings.push(format!(
                    "The franchise was read up to {}: {why}.",
                    chain
                        .seasons
                        .last()
                        .map_or("its root", |s| s.title.as_str())
                ));
            }
            if !chain.seasons.iter().any(|s| s.id == resolved_id) {
                return Ok(p.problem(format!(
                    "The season chain of {} does not reach AniList {resolved_id}; its relations are ambiguous.",
                    group.title
                )));
            }
            let id_set: HashSet<i64> = ids.iter().copied().collect();
            let start = chain
                .seasons
                .iter()
                .position(|s| id_set.contains(&s.id))
                .expect("the resolved id is in the chain");
            let chain_ids: HashSet<i64> = chain.seasons.iter().map(|s| s.id).collect();
            for other in ids.iter().filter(|i| !chain_ids.contains(i)) {
                p.plan.skipped.push(format!(
                    "AniList {other} is not a season of the franchise (a film, OVA, special or another title); it is not converted."
                ));
            }
            if start > 0 {
                p.plan.skipped.push(format!(
                    "Seasons before {} are not part of this series.",
                    chain.seasons[start].title
                ));
            }
            (Some(group), chain, start)
        }
        None if !standalone_ids.is_empty() => {
            // AniList ids grow with time: the standalone entries in release order (the dates
            // are checked when the episodes are mapped).
            let seasons = standalone_ids
                .iter()
                .enumerate()
                .map(|(i, anime)| grouping::ChainSeason {
                    id: *anime,
                    title: format!("AniList {anime}"),
                    ordinal: i as i32 + 1,
                })
                .collect();
            for other in ids.iter().filter(|i| !standalone_ids.contains(i)) {
                p.plan.skipped.push(format!(
                    "AniList {other} is not a TV season (a film, OVA, special or ambiguous title); it is not converted."
                ));
            }
            p.plan.warnings.push(
                "This is a standalone AniList title: it becomes one AniList series without a group, in the same folder, and its files stay where they are.".into(),
            );
            (
                None,
                grouping::SeasonChain {
                    seasons,
                    stopped: None,
                },
                0,
            )
        }
        None => {
            let why = if reasons.is_empty() {
                "none of them is a TV season".to_string()
            } else {
                reasons.join("; ")
            };
            return Ok(p.problem(format!(
                "None of the series' AniList IDs belongs to a franchise of seasons: {why}."
            )));
        }
    };

    // The series' episodes.
    type EpisodeRow = (
        i64,
        i32,
        i32,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        Option<i64>,
    );
    let rows: Vec<EpisodeRow> = sqlx::query_as(
        "SELECT id, season_number, episode_number, title, overview, air_date, air_date_utc, monitored, episode_file_id \
         FROM episodes WHERE series_id = ? ORDER BY season_number, episode_number",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    let episodes: Vec<SourceEpisode> = rows
        .into_iter()
        .map(
            |(id, season, episode, title, overview, air_date, air_date_utc, monitored, file_id)| {
                SourceEpisode {
                    id,
                    season,
                    episode,
                    title,
                    overview,
                    air_date,
                    air_date_utc,
                    monitored,
                    file_id,
                }
            },
        )
        .collect();
    let specials: Vec<String> = episodes
        .iter()
        .filter(|e| e.season == 0 && e.file_id.is_some())
        .map(SourceEpisode::label)
        .collect();
    if !specials.is_empty() {
        p = p.problem(format!(
            "Specials have files ({}): AniList OVAs and specials are not converted. Move or delete those files first.",
            listed(specials)
        ));
    }
    let regular: Vec<SourceEpisode> = episodes.into_iter().filter(|e| e.season > 0).collect();
    if regular.is_empty() {
        return Ok(p.problem("The series has no episodes to convert."));
    }

    // The entries' episodes, as adding each AniList title would create them.
    let language = crate::metadata_language_code(crate::preferred_metadata_language(db).await);
    let mut entries = Vec::new();
    let mut infos = Vec::new();
    let mut covered = 0usize;
    for season in &chain.seasons[start..] {
        if covered >= regular.len() {
            break;
        }
        let info = match metadata
            .series_ordered_in(
                MetadataProvider::AniList,
                &season.id.to_string(),
                None,
                Some(&language),
            )
            .await
        {
            Ok(i) => i,
            Err(e) => {
                return Ok(p.problem(format!("AniList {} could not be read: {e}", season.id)));
            }
        };
        let count = info.episodes.len();
        entries.push(Entry {
            id: season.id,
            title: info.title.clone(),
            ordinal: season.ordinal,
            count: (count > 0).then_some(count),
            first_air: info
                .episodes
                .first()
                .and_then(|e| e.air_date.clone())
                .or_else(|| info.first_aired.clone()),
        });
        infos.push(info);
        if count == 0 {
            break;
        }
        covered += count;
    }
    let more_in_chain = start + entries.len() < chain.seasons.len();
    let mapping = match map_episodes(&regular, &entries, more_in_chain) {
        Ok(m) => m,
        Err(why) => return Ok(p.problem(why)),
    };
    p.plan.warnings.extend(mapping.warnings.iter().cloned());
    if !mapping.leftover.is_empty() {
        let left: Vec<&SourceEpisode> = mapping.leftover.iter().map(|i| &regular[*i]).collect();
        p.plan.skipped.push(format!(
            "{} episodes without files beyond the AniList seasons are not carried over ({}).",
            left.len(),
            range(&left)
        ));
    }

    // Members: the followed entries only.
    let naming: NamingSettings = settings::get_or_default(db.pool(), settings::KEY_NAMING).await?;
    let (group_kind, style) = match &group {
        Some(g) => {
            grouping::stored_folder_settings(db, MetadataProvider::AniList, &g.root.to_string())
                .await
        }
        None => Default::default(),
    };
    for chunk in &mapping.chunks {
        let entry = &entries[chunk.entry];
        let mapped: Vec<&SourceEpisode> = chunk.episodes.iter().map(|i| &regular[*i]).collect();
        if !followed(&regular, chunk) {
            p.plan.skipped.push(format!(
                "Season {} · {} (AniList {}): {} have no files and are not monitored; the season is not added.",
                entry.ordinal,
                entry.title,
                entry.id,
                range(&mapped)
            ));
            continue;
        }
        let info = infos[chunk.entry].clone();
        let existing: Option<(i64, String)> = sqlx::query_as(
            "SELECT id, path FROM series WHERE metadata_provider = ? AND metadata_id = ?",
        )
        .bind(MetadataProvider::AniList.as_str())
        .bind(entry.id.to_string())
        .fetch_optional(db.pool())
        .await?;
        let path = match (&existing, &group) {
            (Some((_, path)), _) => path.clone(),
            (None, None) => source.path.clone(),
            (None, Some(group)) => {
                let membership = crate::file_names::Series {
                    title: info.title.clone(),
                    group: Some(crate::file_names::GroupMembership {
                        title: group.title.clone(),
                        season: Some(entry.ordinal),
                        movie: None,
                    }),
                    ..Default::default()
                };
                let format = grouping::member_folder_format(
                    &naming,
                    &group_kind,
                    &style,
                    false,
                    crate::file_names::is_story_arc(&info.title, &group.title),
                );
                let folder = crate::file_names::group_folder(format, &membership, &naming);
                if folder.is_empty() {
                    p = p.problem(format!(
                        "The member folder format gives {} an empty folder name.",
                        info.title
                    ));
                    continue;
                }
                kitsunarr_domain::naming::join(&source.path, &folder)
            }
        };
        if existing.is_none() {
            let used: Option<i64> =
                sqlx::query_scalar("SELECT id FROM series WHERE path = ? AND id <> ?")
                    .bind(&path)
                    .bind(id)
                    .fetch_optional(db.pool())
                    .await?;
            if used.is_some() {
                p = p.problem(format!("{path} is already the folder of another series."));
            }
        }
        if p.members.iter().any(|m| m.path == path) {
            p = p.problem(format!("Two seasons would share the folder {path}."));
        }
        let index = p.members.len();
        for (k, e) in chunk.episodes.iter().enumerate() {
            p.episode_member
                .insert(regular[*e].id, (index, k as i32 + 1));
        }
        if let Some((existing_id, _)) = &existing {
            let taken: Vec<(i32, Option<i64>)> = sqlx::query_as(
                "SELECT episode_number, episode_file_id FROM episodes WHERE series_id = ? AND season_number = 1",
            )
            .bind(existing_id)
            .fetch_all(db.pool())
            .await?;
            let both: Vec<String> = chunk
                .episodes
                .iter()
                .enumerate()
                .filter(|(k, e)| {
                    regular[**e].file_id.is_some()
                        && taken
                            .iter()
                            .any(|(n, f)| *n == *k as i32 + 1 && f.is_some())
                })
                .map(|(k, _)| format!("{}", k + 1))
                .collect();
            if !both.is_empty() {
                p = p.problem(format!(
                    "{} is already in the library with files for episodes {} that this series also has.",
                    info.title,
                    listed(both)
                ));
            }
            p.plan.warnings.push(format!(
                "{} is already in the library; its episodes receive this series' files.",
                info.title
            ));
        }
        p.members.push(PreparedMember {
            entry: entry.clone(),
            info,
            source: chunk.episodes.clone(),
            existing,
            path,
        });
    }
    if p.members.is_empty() && p.plan.problems.is_empty() {
        return Ok(
            p.problem("No season has files or monitored episodes: there is nothing to convert.")
        );
    }
    if p.members.len() > 1 && group.is_none() {
        let titles: Vec<String> = p.members.iter().map(|m| m.info.title.clone()).collect();
        p = p.problem(format!(
            "Several standalone AniList titles have files or monitored episodes ({}): convert them with Change catalogue one at a time.",
            titles.join(", ")
        ));
    }
    if let Some(group) = &group {
        let collection_paths: Vec<String> = sqlx::query_scalar(
            "SELECT s.path FROM collection_members m JOIN collections c ON c.id = m.collection_id \
             JOIN series s ON s.id = m.series_id WHERE c.kind = 'animeGroup' AND c.metadata_provider = ? AND c.metadata_id = ?",
        )
        .bind(MetadataProvider::AniList.as_str())
        .bind(group.root.to_string())
        .fetch_all(db.pool())
        .await?;
        for other in collection_paths {
            let parent = Path::new(&other).parent().map(Path::to_path_buf);
            if parent.as_deref() != Some(Path::new(&source.path)) {
                p.plan.warnings.push(format!(
                    "{other} is a member of this group outside {}; it stays where it is.",
                    source.path
                ));
            }
        }
    }

    // Files.
    let file_rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, relative_path FROM episode_files WHERE series_id = ?")
            .bind(id)
            .fetch_all(db.pool())
            .await?;
    p.file_count = file_rows.len() as i64;
    let mut by_file: BTreeMap<i64, Vec<&SourceEpisode>> = BTreeMap::new();
    for e in &regular {
        if let Some(f) = e.file_id {
            by_file.entry(f).or_default().push(e);
        }
    }
    let mut unlinked = 0;
    for (file_id, relative) in &file_rows {
        let Some(eps) = by_file.get(file_id) else {
            unlinked += 1;
            continue;
        };
        let members: HashSet<usize> = eps
            .iter()
            .filter_map(|e| p.episode_member.get(&e.id).map(|(m, _)| *m))
            .collect();
        if members.len() != 1 || eps.iter().any(|e| !p.episode_member.contains_key(&e.id)) {
            p = p.problem(format!(
                "{relative} holds episodes of several AniList seasons ({}).",
                range(eps)
            ));
            continue;
        }
        let member = *members.iter().next().expect("one member");
        let name = Path::new(relative)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| relative.clone());
        // A standalone title keeps the series' layout: the same relative path (in the same
        // folder, or in the folder of the library series that already owns the entry).
        let new_relative = if group.is_some() {
            name
        } else {
            relative.clone()
        };
        p.files.push(FilePlan {
            id: *file_id,
            member,
            relative_path: relative.clone(),
            new_relative,
            episodes: eps.iter().map(|e| e.id).collect(),
        });
    }
    if unlinked > 0 {
        p.plan.warnings.push(format!(
            "{unlinked} file records are not linked to an episode; they are dropped (the files stay on disk)."
        ));
    }
    let disk = {
        let base = PathBuf::from(&source.path);
        let files: Vec<(PathBuf, PathBuf)> = p
            .files
            .iter()
            .map(|f| {
                let target = PathBuf::from(&p.members[f.member].path);
                let folder = match Path::new(&f.new_relative).parent() {
                    Some(dir) if group.is_none() => target.join(dir),
                    _ => target,
                };
                (base.join(&f.relative_path), folder)
            })
            .collect();
        tokio::task::spawn_blocking(move || plan_moves(&files))
            .await
            .map_err(|e| LibraryError::invalid("dryRun", e.to_string()))?
    };
    for missing in &disk.missing {
        p.plan.warnings.push(format!(
            "{} is missing on disk; its record still follows the episode.",
            missing.display()
        ));
    }
    for conflict in disk.conflicts {
        p = p.problem(conflict);
    }
    p.moves = disk.moves;
    for (from, to) in &p.moves {
        p.plan.moves.push(AnimeConversionMove {
            from: from.display().to_string(),
            to: to.display().to_string(),
        });
    }
    // Records already in a reused member that the moved files would collide with.
    let mut clashes = Vec::new();
    for f in &p.files {
        if let Some((existing, _)) = &p.members[f.member].existing {
            let clash: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM episode_files WHERE series_id = ? AND relative_path = ?)",
            )
            .bind(existing)
            .bind(&f.new_relative)
            .fetch_one(db.pool())
            .await?;
            if clash {
                clashes.push(format!(
                    "{} already has a file record named {}.",
                    p.members[f.member].info.title, f.new_relative
                ));
            }
        }
    }
    for clash in clashes {
        p = p.problem(clash);
    }

    // Extra and metadata files follow their episode file when they end up in its folder.
    let moved: HashMap<PathBuf, PathBuf> = p.moves.iter().cloned().collect();
    let file_member: HashMap<i64, usize> = p.files.iter().map(|f| (f.id, f.member)).collect();
    let base = PathBuf::from(&source.path);
    let mut dropped = 0;
    for (table, followers) in [("extra_files", 0usize), ("metadata_files", 1usize)] {
        let rows: Vec<(i64, Option<i64>, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, episode_file_id, relative_path FROM {table} WHERE series_id = ?"
        )))
        .bind(id)
        .fetch_all(db.pool())
        .await?;
        for (row, file, relative) in rows {
            let Some(member) = file.and_then(|f| file_member.get(&f).copied()) else {
                dropped += 1;
                continue;
            };
            let old = base.join(&relative);
            let new = moved.get(&old).cloned().unwrap_or(old);
            let Ok(rest) = new.strip_prefix(&p.members[member].path) else {
                dropped += 1;
                continue;
            };
            let follower = Follower {
                id: row,
                member,
                new_relative: rest.to_string_lossy().replace('\\', "/"),
            };
            if followers == 0 {
                p.extras.push(follower);
            } else {
                p.metadata_files.push(follower);
            }
        }
    }
    if dropped > 0 {
        p.plan.warnings.push(format!(
            "{dropped} extra or metadata file records of the series folder are dropped (the files stay in {}).",
            source.path
        ));
    }

    // Kept versions follow when all their episodes go to one new member below the group folder.
    type VersionRow = (i64, String, String, String);
    let versions: Vec<VersionRow> = sqlx::query_as(
        "SELECT id, relative_path, archive_path, episode_ids FROM retained_versions WHERE series_id = ?",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    let mut lost_versions = 0;
    for (row, relative, archive, episode_ids) in versions {
        let eps: Vec<i64> = serde_json::from_str(&episode_ids).unwrap_or_default();
        let members: HashSet<usize> = eps
            .iter()
            .filter_map(|e| p.episode_member.get(e).map(|(m, _)| *m))
            .collect();
        let member = match (members.len(), members.iter().next()) {
            (1, Some(m)) if eps.iter().all(|e| p.episode_member.contains_key(e)) => *m,
            _ => {
                lost_versions += 1;
                continue;
            }
        };
        let m = &p.members[member];
        let takes_over = m.path == source.path;
        if m.existing.is_some()
            || (!takes_over && Path::new(&m.path).parent() != Some(Path::new(&source.path)))
        {
            lost_versions += 1;
            continue;
        }
        p.versions.push(Version {
            id: row,
            member,
            relative_path: relative,
            archive_path: archive,
            episode_ids: eps,
        });
    }
    if lost_versions > 0 {
        p.plan.warnings.push(format!(
            "{lost_versions} kept versions are no longer listed (their files stay in {}).",
            source.path
        ));
    }
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pending_releases WHERE series_id = ?")
            .bind(id)
            .fetch_one(db.pool())
            .await?;
    if pending > 0 {
        p.plan.warnings.push(format!(
            "{pending} delayed releases of the series are dropped; the next RSS sync finds them again."
        ));
    }

    // The report.
    for m in &p.members {
        let mapped: Vec<&SourceEpisode> = m.source.iter().map(|i| &regular[*i]).collect();
        p.plan.members.push(AnimeConversionMember {
            anilist_id: m.entry.id,
            title: m.info.title.clone(),
            ordinal: m.entry.ordinal,
            path: m.path.clone(),
            episodes: m.info.episodes.len().max(m.source.len()) as i32,
            source_episodes: range(&mapped),
            files: mapped
                .iter()
                .filter_map(|e| e.file_id)
                .collect::<HashSet<_>>()
                .len() as i32,
            monitored_episodes: mapped.iter().filter(|e| e.monitored).count() as i32,
            existing_series_id: m.existing.as_ref().map(|(id, _)| *id),
            series_id: None,
        });
    }
    p.regular = regular;
    p.plan.convertible = p.plan.problems.is_empty();
    Ok(p)
}

#[derive(Default)]
struct DiskPlan {
    /// (from, to), the episode files and their companions.
    moves: Vec<(PathBuf, PathBuf)>,
    missing: Vec<PathBuf>,
    conflicts: Vec<String>,
}

/// The renames that put each file (and its companions: files in its folder named after it,
/// such as subtitles) into its member's folder. A destination that exists, or that two files
/// would share, is a conflict.
fn plan_moves(files: &[(PathBuf, PathBuf)]) -> DiskPlan {
    let mut out = DiskPlan::default();
    let episode_files: HashSet<&PathBuf> = files.iter().map(|(f, _)| f).collect();
    let mut listing: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut targets = HashSet::new();
    for (file, folder) in files {
        let (Some(dir), Some(name), Some(stem)) =
            (file.parent(), file.file_name(), file.file_stem())
        else {
            continue;
        };
        if !file.is_file() {
            out.missing.push(file.clone());
            continue;
        }
        if dir == folder.as_path() {
            continue;
        }
        let prefix = format!("{}.", stem.to_string_lossy());
        let names = listing.entry(dir.to_path_buf()).or_insert_with(|| {
            std::fs::read_dir(dir)
                .map(|it| {
                    it.filter_map(Result::ok)
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default()
        });
        let mut group = vec![name.to_string_lossy().into_owned()];
        group.extend(
            names
                .iter()
                .filter(|n| n.starts_with(&prefix) && n.as_str() != name.to_string_lossy())
                .filter(|n| !episode_files.contains(&dir.join(n.as_str())))
                .cloned(),
        );
        for n in group {
            let (from, to) = (dir.join(&n), folder.join(&n));
            if to.exists() {
                out.conflicts
                    .push(format!("{} already exists.", to.display()));
            } else if !targets.insert(to.clone()) {
                out.conflicts
                    .push(format!("Two files would be moved to {}.", to.display()));
            }
            out.moves.push((from, to));
        }
    }
    out
}

/// Renames the files; on a failure the files already moved are put back.
fn perform_moves(moves: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut done: Vec<&(PathBuf, PathBuf)> = Vec::new();
    for m in moves {
        let (from, to) = m;
        let result = to
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                if to.exists() {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "the destination exists",
                    ))
                } else {
                    std::fs::rename(from, to)
                }
            });
        if let Err(e) = result {
            undo_moves(&done);
            return Err(format!(
                "Moving {} to {} failed: {e}",
                from.display(),
                to.display()
            ));
        }
        done.push(m);
    }
    Ok(())
}

fn undo_moves(done: &[&(PathBuf, PathBuf)]) {
    for (from, to) in done.iter().rev() {
        if let Err(e) = std::fs::rename(to, from) {
            tracing::error!(from = %to.display(), to = %from.display(), error = %e, "a moved file could not be put back");
        }
    }
}

/// Removes the folders the files left when they are empty (never the group folder itself).
fn remove_emptied(moves: &[(PathBuf, PathBuf)], keep: &Path) {
    let dirs: HashSet<&Path> = moves.iter().filter_map(|(f, _)| f.parent()).collect();
    for d in dirs {
        if d != keep && d.starts_with(keep) {
            let _ = std::fs::remove_dir(d);
        }
    }
}

/// Plans the conversion again and carries it out.
async fn apply(db: &Database, metadata: &Metadata, id: i64) -> LibraryResult<AnimeConversionPlan> {
    let mut p = prepare(db, metadata, id).await?;
    if !p.plan.convertible {
        return Ok(p.plan);
    }
    // Missing presentation is completed as when adding.
    for m in p.members.iter_mut().filter(|m| m.existing.is_none()) {
        metadata.enrich(&mut m.info).await;
    }
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let ids = write(&mut tx, &p).await?;
    let moves = p.moves.clone();
    let moved = tokio::task::spawn_blocking(move || perform_moves(&moves))
        .await
        .map_err(|e| LibraryError::invalid("dryRun", e.to_string()))?;
    if let Err(why) = moved {
        drop(tx);
        p.plan.problems.push(format!("{why}. Nothing was changed."));
        return Ok(p.plan);
    }
    if let Err(e) = tx.commit().await {
        let moves = p.moves.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let refs: Vec<&(PathBuf, PathBuf)> = moves.iter().collect();
            undo_moves(&refs);
        })
        .await;
        return Err(e.into());
    }
    let (moves, keep) = (p.moves.clone(), PathBuf::from(&p.plan.group_path));
    let _ = tokio::task::spawn_blocking(move || remove_emptied(&moves, &keep)).await;
    for (member, id) in p.plan.members.iter_mut().zip(&ids) {
        member.series_id = Some(*id);
    }
    p.plan.applied = true;
    tracing::info!(
        series = %p.plan.title,
        members = ids.len(),
        files = p.files.len(),
        "series converted into AniList group members"
    );
    Ok(p.plan)
}

/// The conversion's database writes; returns the members' series ids.
async fn write(tx: &mut SqliteConnection, p: &Prepared) -> LibraryResult<Vec<i64>> {
    let source = p
        .source
        .as_ref()
        .expect("a convertible plan has its source");
    let group = p.group.as_ref();
    // The series leaves its folder free for a standalone title that takes it over; it is
    // deleted at the end.
    sqlx::query("UPDATE series SET path = path || ' (converting)' WHERE id = ?")
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
    let files_now: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM episode_files WHERE series_id = ?")
            .bind(source.id)
            .fetch_one(&mut *tx)
            .await?;
    if files_now != p.file_count {
        return Err(LibraryError::invalid(
            "dryRun",
            "The series' files changed while converting; try again.",
        ));
    }
    let tags: Vec<i64> = sqlx::query_scalar("SELECT tag_id FROM series_tags WHERE series_id = ?")
        .bind(source.id)
        .fetch_all(&mut *tx)
        .await?;
    let monitor_new = if source.monitor_new_items == MonitorNewItems::None.as_str() {
        MonitorNewItems::None
    } else {
        MonitorNewItems::All
    };
    let now = kitsunarr_db::now();
    let mut ids = Vec::new();
    // Old episode id → (member series, new episode id).
    let mut episode_map: HashMap<i64, (i64, i64)> = HashMap::new();
    for (index, m) in p.members.iter().enumerate() {
        let mapped: HashMap<i32, &SourceEpisode> = m
            .source
            .iter()
            .enumerate()
            .map(|(k, i)| (k as i32 + 1, &p.regular[*i]))
            .collect();
        let series_id = match &m.existing {
            Some((existing, _)) => *existing,
            None => {
                let info = &m.info;
                let created = series::insert(
                    &mut *tx,
                    &series::NewSeries {
                        media_kind: MediaKind::Anime,
                        title: info.title.clone(),
                        year: info.year,
                        status: info.status,
                        overview: info.overview.clone(),
                        network: info.network.clone(),
                        air_time: info.air_time.clone(),
                        runtime: info.runtime,
                        certification: None,
                        original_language: info.original_language,
                        genres: info.genres.clone(),
                        images: info.images.clone(),
                        ratings: crate::ratings(info),
                        first_aired: info.first_aired.clone(),
                        last_aired: info.last_aired.clone(),
                        series_type: SeriesType::Anime,
                        metadata_provider: MetadataProvider::AniList,
                        metadata_id: info.id.clone(),
                        episode_ordering: None,
                        monitored: source.monitored,
                        monitor_new_items: monitor_new,
                        // Group members are season folders themselves; a standalone title keeps
                        // the series' setting.
                        season_folder: group.is_none() && source.season_folder,
                        use_scene_numbering: false,
                        quality_profile_id: source.quality_profile_id,
                        root_folder_path: source.root_folder_path.clone(),
                        path: m.path.clone(),
                        added_at: now.clone(),
                        last_refreshed_at: Some(now.clone()),
                        tags: tags.clone(),
                        external_ids: info.external_ids.clone(),
                        alternate_titles: info
                            .alternate_titles
                            .iter()
                            .map(|t| (t.clone(), None, Some("provider".to_string())))
                            .collect(),
                    },
                )
                .await?;
                sqlx::query("UPDATE series SET keep_versions = ?, renaming_mode = ? WHERE id = ?")
                    .bind(source.keep_versions)
                    .bind(&source.renaming_mode)
                    .bind(created)
                    .execute(&mut *tx)
                    .await?;
                series::set_original_country(&mut *tx, created, info.original_country.as_deref())
                    .await?;
                // AniList's episodes, or (count unknown) the series' own for this season.
                let episodes: Vec<EpisodeInfo> = if info.episodes.is_empty() {
                    m.source
                        .iter()
                        .enumerate()
                        .map(|(k, i)| {
                            let e = &p.regular[*i];
                            let n = k as i32 + 1;
                            EpisodeInfo {
                                season_number: 1,
                                episode_number: n,
                                absolute_episode_number: Some(n),
                                title: e.title.clone(),
                                overview: e.overview.clone(),
                                air_date: e.air_date.clone(),
                                air_date_utc: e.air_date_utc.clone(),
                                runtime: info.runtime,
                                provider_episode_id: Some(format!("{}:{n}", info.id)),
                                image: None,
                            }
                        })
                        .collect()
                } else {
                    info.episodes.clone()
                };
                for e in &episodes {
                    let from = mapped.get(&e.episode_number);
                    series::upsert_episode(
                        &mut *tx,
                        &series::NewEpisode {
                            series_id: created,
                            season_number: e.season_number,
                            episode_number: e.episode_number,
                            absolute_episode_number: e.absolute_episode_number,
                            scene_season_number: None,
                            scene_episode_number: None,
                            scene_absolute_episode_number: None,
                            unverified_scene_numbering: false,
                            // AniList has no episode titles: the series' own are kept.
                            title: e
                                .title
                                .clone()
                                .or_else(|| from.and_then(|f| f.title.clone())),
                            overview: e
                                .overview
                                .clone()
                                .or_else(|| from.and_then(|f| f.overview.clone())),
                            air_date: e.air_date.clone(),
                            air_date_utc: e.air_date_utc.clone(),
                            runtime: e.runtime,
                            monitored: from
                                .map_or(monitor_new == MonitorNewItems::All, |f| f.monitored),
                            episode_file_id: None,
                            provider_episode_id: e.provider_episode_id.clone(),
                            last_search_at: None,
                        },
                    )
                    .await?;
                    series::set_episode_screenshot(
                        &mut *tx,
                        created,
                        e.season_number,
                        e.episode_number,
                        e.image.as_deref(),
                    )
                    .await?;
                }
                kitsunarr_db::auto_tags::apply_series(&mut *tx, created).await?;
                created
            }
        };
        // Episodes the existing member lacks are added; monitored episodes stay monitored.
        let present: Vec<(i64, i32)> = sqlx::query_as(
            "SELECT id, episode_number FROM episodes WHERE series_id = ? AND season_number = 1",
        )
        .bind(series_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut by_number: HashMap<i32, i64> = present.into_iter().map(|(i, n)| (n, i)).collect();
        for (n, from) in &mapped {
            let episode = match by_number.get(n) {
                Some(e) => *e,
                None => {
                    let e = series::upsert_episode(
                        &mut *tx,
                        &series::NewEpisode {
                            series_id,
                            season_number: 1,
                            episode_number: *n,
                            absolute_episode_number: Some(*n),
                            scene_season_number: None,
                            scene_episode_number: None,
                            scene_absolute_episode_number: None,
                            unverified_scene_numbering: false,
                            title: from.title.clone(),
                            overview: from.overview.clone(),
                            air_date: from.air_date.clone(),
                            air_date_utc: from.air_date_utc.clone(),
                            runtime: m.info.runtime,
                            monitored: from.monitored,
                            episode_file_id: None,
                            provider_episode_id: Some(format!("{}:{n}", m.entry.id)),
                            last_search_at: None,
                        },
                    )
                    .await?;
                    by_number.insert(*n, e);
                    e
                }
            };
            // Watched state, last search and monitoring carry over.
            sqlx::query(
                "UPDATE episodes AS e SET watched = MAX(e.watched, s.watched), \
                 watched_at = COALESCE(e.watched_at, s.watched_at), watched_source = COALESCE(e.watched_source, s.watched_source), \
                 last_search_at = COALESCE(e.last_search_at, s.last_search_at), \
                 monitored = CASE WHEN ?3 THEN e.monitored OR s.monitored ELSE s.monitored END \
                 FROM episodes AS s WHERE e.id = ?1 AND s.id = ?2",
            )
            .bind(episode)
            .bind(from.id)
            .bind(m.existing.is_some())
            .execute(&mut *tx)
            .await?;
            episode_map.insert(from.id, (series_id, episode));
        }
        let season_monitored: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM episodes WHERE series_id = ? AND season_number = 1 AND monitored = 1)",
        )
        .bind(series_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO seasons (series_id, season_number, monitored) VALUES (?, 1, ?) \
             ON CONFLICT(series_id, season_number) DO UPDATE SET monitored = excluded.monitored OR seasons.monitored",
        )
        .bind(series_id)
        .bind(season_monitored)
        .execute(&mut *tx)
        .await?;
        if let Some(group) = group {
            grouping::join_in(
                &mut *tx,
                MetadataProvider::AniList,
                &Group {
                    id: group.id.clone(),
                    root: group.root,
                    title: group.title.clone(),
                    member_kind: MemberKind::Season,
                    ordinal: m.entry.ordinal,
                },
                grouping::Member::Series(series_id),
            )
            .await?;
        }
        debug_assert_eq!(index, ids.len());
        ids.push(series_id);
    }

    // Episode files and the records that follow them.
    for f in &p.files {
        let member = ids[f.member];
        sqlx::query(
            "UPDATE episode_files SET series_id = ?, season_number = 1, relative_path = ? WHERE id = ?",
        )
        .bind(member)
        .bind(&f.new_relative)
        .bind(f.id)
        .execute(&mut *tx)
        .await
        .map_err(|e| DbError::from_write(e, "episode file"))?;
        for old in &f.episodes {
            if let Some((_, new)) = episode_map.get(old) {
                sqlx::query("UPDATE episodes SET episode_file_id = ? WHERE id = ?")
                    .bind(f.id)
                    .bind(new)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    for (table, rows) in [
        ("extra_files", &p.extras),
        ("metadata_files", &p.metadata_files),
    ] {
        for r in rows {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET series_id = ?, season_number = 1, relative_path = ? WHERE id = ?"
            )))
            .bind(ids[r.member])
            .bind(&r.new_relative)
            .bind(r.id)
            .execute(&mut *tx)
            .await?;
        }
    }
    for v in &p.versions {
        let new_ids: Vec<i64> = v
            .episode_ids
            .iter()
            .filter_map(|e| episode_map.get(e).map(|(_, n)| *n))
            .collect();
        let name = Path::new(&v.relative_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| v.relative_path.clone());
        // The archive stays in the group folder, one level above the member's folder; a
        // standalone title keeps the series' folder and paths.
        let (relative, archive) = if group.is_some() {
            (name, format!("../{}", v.archive_path))
        } else {
            (v.relative_path.clone(), v.archive_path.clone())
        };
        sqlx::query(
            "UPDATE retained_versions SET series_id = ?, relative_path = ?, archive_path = ?, episode_ids = ? WHERE id = ?",
        )
        .bind(ids[v.member])
        .bind(relative)
        .bind(archive)
        .bind(serde_json::to_string(&new_ids).unwrap_or_else(|_| "[]".into()))
        .bind(v.id)
        .execute(&mut *tx)
        .await?;
    }

    // History (and with it the queue, which reads grabs from history).
    let history: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, episode_id FROM history WHERE series_id = ?")
            .bind(source.id)
            .fetch_all(&mut *tx)
            .await?;
    for (row, episode) in history {
        let target = match episode {
            Some(e) => episode_map.get(&e).map(|(s, n)| (*s, Some(*n))),
            None => ids.first().map(|s| (*s, None)),
        };
        if let Some((series_id, episode_id)) = target {
            sqlx::query("UPDATE history SET series_id = ?, episode_id = ? WHERE id = ?")
                .bind(series_id)
                .bind(episode_id)
                .bind(row)
                .execute(&mut *tx)
                .await?;
        }
    }
    // Blocklist entries go to each member their episodes belong to.
    let blocked: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, episode_ids FROM blocklist WHERE series_id = ?")
            .bind(source.id)
            .fetch_all(&mut *tx)
            .await?;
    for (row, episode_ids) in blocked {
        let old: Vec<i64> = serde_json::from_str(&episode_ids).unwrap_or_default();
        let mut per_member: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for e in &old {
            if let Some((s, n)) = episode_map.get(e) {
                per_member.entry(*s).or_default().push(*n);
            }
        }
        if old.is_empty()
            && let Some(first) = ids.first()
        {
            per_member.insert(*first, Vec::new());
        }
        for (k, (series_id, episodes)) in per_member.into_iter().enumerate() {
            let json = serde_json::to_string(&episodes).unwrap_or_else(|_| "[]".into());
            if k == 0 {
                sqlx::query("UPDATE blocklist SET series_id = ?, episode_ids = ? WHERE id = ?")
                    .bind(series_id)
                    .bind(json)
                    .bind(row)
                    .execute(&mut *tx)
                    .await?;
            } else {
                sqlx::query(
                    "INSERT INTO blocklist (series_id, movie_id, episode_ids, source_title, quality_id, revision, languages, date, \
                     published_date, size, protocol, indexer, indexer_flags, release_type, message, source, torrent_info_hash) \
                     SELECT ?, NULL, ?, source_title, quality_id, revision, languages, date, published_date, size, protocol, indexer, \
                     indexer_flags, release_type, message, source, torrent_info_hash FROM blocklist WHERE id = ?",
                )
                .bind(series_id)
                .bind(json)
                .bind(row)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    // The series' names help releases find the members: the franchise's names belong to its
    // first season (when it is a member), season-specific ones to the member holding that
    // season's first episode. Elsewhere they would send other seasons' releases to the wrong
    // member.
    let first_season = p
        .members
        .iter()
        .position(|m| m.entry.ordinal == 1)
        .map(|i| ids[i]);
    let titles: Vec<(String, Option<i32>)> = sqlx::query_as(
        "SELECT title, NULL FROM series WHERE id = ?1 UNION ALL \
         SELECT title, season_number FROM series_alternate_titles WHERE series_id = ?1",
    )
    .bind(source.id)
    .fetch_all(&mut *tx)
    .await?;
    for (title, season) in titles {
        let member = match season {
            None => first_season,
            Some(n) => p
                .regular
                .iter()
                .find(|e| e.season == n)
                .and_then(|e| episode_map.get(&e.id))
                .map(|(s, _)| *s),
        };
        if let Some(member) = member {
            sqlx::query(
                "INSERT OR IGNORE INTO series_alternate_titles (series_id, title, clean_title, season_number, source) \
                 VALUES (?, ?, ?, NULL, 'tvdb')",
            )
            .bind(member)
            .bind(title.trim())
            .bind(kitsunarr_domain::text::clean_title(&title))
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query("DELETE FROM series WHERE id = ?")
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seasons`: episodes per TheTVDB season, starting on the given dates (weekly).
    fn tvdb(seasons: &[(i32, usize, &str)]) -> Vec<SourceEpisode> {
        let mut out = Vec::new();
        let mut id = 1;
        for (season, count, start) in seasons {
            let first = NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap();
            for n in 0..*count {
                out.push(SourceEpisode {
                    id,
                    season: *season,
                    episode: n as i32 + 1,
                    air_date: Some((first + chrono::Days::new(7 * n as u64)).to_string()),
                    ..Default::default()
                });
                id += 1;
            }
        }
        out
    }

    fn entry(id: i64, ordinal: i32, count: Option<usize>, first: &str) -> Entry {
        Entry {
            id,
            title: format!("Entry {id}"),
            ordinal,
            count,
            first_air: Some(first.into()),
        }
    }

    fn spans(m: &Mapping) -> Vec<(usize, usize, usize)> {
        m.chunks
            .iter()
            .map(|c| {
                (
                    c.entry,
                    c.episodes.first().copied().unwrap_or(0),
                    c.episodes.len(),
                )
            })
            .collect()
    }

    #[test]
    fn seasons_map_sequentially_onto_entries() {
        let eps = tvdb(&[(1, 11, "2023-04-12"), (2, 13, "2024-07-03")]);
        let entries = [
            entry(150672, 1, Some(11), "2023-04-12"),
            entry(166531, 2, Some(13), "2024-07-03"),
        ];
        let m = map_episodes(&eps, &entries, false).unwrap();
        assert_eq!(spans(&m), vec![(0, 0, 11), (1, 11, 13)]);
        assert!(m.leftover.is_empty());
    }

    /// TheTVDB season 3 is two AniList parts (split cour).
    #[test]
    fn a_split_season_maps_onto_two_entries() {
        let eps = tvdb(&[(1, 12, "2020-01-05"), (3, 24, "2022-01-09")]);
        // Part 2 premieres after a 12-week break, but TheTVDB lists it as weekly; its first
        // episode's date is 12 weeks after the season's first: 2022-04-03.
        let entries = [
            entry(1, 1, Some(12), "2020-01-05"),
            entry(2, 2, Some(12), "2022-01-09"),
            entry(3, 3, Some(12), "2022-04-03"),
        ];
        let m = map_episodes(&eps, &entries, false).unwrap();
        assert_eq!(spans(&m), vec![(0, 0, 12), (1, 12, 12), (2, 24, 12)]);
        assert_eq!(eps[24].label(), "S03E13");
    }

    #[test]
    fn placeholders_without_files_may_be_left_over() {
        let mut eps = tvdb(&[(1, 12, "2020-01-05"), (2, 12, "2026-12-01")]);
        let entries = [entry(1, 1, Some(12), "2020-01-05")];
        let m = map_episodes(&eps, &entries, false).unwrap();
        assert_eq!(m.leftover.len(), 12);
        // A file among them makes the series not convertible.
        eps[15].file_id = Some(9);
        let err = map_episodes(&eps, &entries, false).unwrap_err();
        assert!(err.contains("S02E04"), "{err}");
    }

    #[test]
    fn a_streaming_premiere_at_a_season_start_is_accepted_with_a_warning() {
        // JoJo's Stone Ocean: Netflix premiere five weeks before TheTVDB's broadcast date.
        let eps = tvdb(&[(1, 12, "2018-01-05"), (2, 12, "2022-01-08")]);
        let entries = [
            entry(1, 1, Some(12), "2018-01-05"),
            entry(2, 2, Some(12), "2021-12-01"),
        ];
        let m = map_episodes(&eps, &entries, false).expect("season-aligned");
        assert_eq!(spans(&m), vec![(0, 0, 12), (1, 12, 12)]);
        assert_eq!(m.warnings.len(), 1);
        assert!(m.warnings[0].contains("S02E01"), "{:?}", m.warnings);
    }

    #[test]
    fn a_season_released_in_batches_keeps_its_gap() {
        // Stone Ocean: TheTVDB season 5 holds three AniList parts, each released on Netflix
        // about five weeks before the broadcast.
        let eps = tvdb(&[(5, 24, "2022-01-08")]);
        let entries = [
            entry(1, 1, Some(12), "2021-12-01"),
            entry(2, 2, Some(12), "2022-02-24"),
        ];
        let m = map_episodes(&eps, &entries, false).expect("same gap as the season start");
        assert_eq!(spans(&m), vec![(0, 0, 12), (1, 12, 12)]);
    }

    #[test]
    fn a_mismatched_premiere_in_mid_season_still_rejects() {
        // The second entry would start at S01E07: not a season start.
        let eps = tvdb(&[(1, 12, "2020-01-05")]);
        let entries = [
            entry(1, 1, Some(6), "2020-01-05"),
            entry(2, 2, Some(6), "2021-06-01"),
        ];
        assert!(map_episodes(&eps, &entries, false).is_err());
    }

    #[test]
    fn dates_that_do_not_line_up_are_rejected() {
        let eps = tvdb(&[(1, 13, "2020-01-05"), (2, 12, "2021-01-10")]);
        // AniList counts one episode fewer: season 2's chunk starts on S01E13.
        let entries = [
            entry(1, 1, Some(12), "2020-01-05"),
            entry(2, 2, Some(12), "2021-01-10"),
        ];
        let err = map_episodes(&eps, &entries, false).unwrap_err();
        assert!(err.contains("S01E13"), "{err}");
        // Within three weeks is accepted.
        let entries = [
            entry(1, 1, Some(13), "2020-01-20"),
            entry(2, 2, Some(12), "2020-12-25"),
        ];
        assert!(map_episodes(&eps, &entries, false).is_ok());
    }

    #[test]
    fn an_unknown_count_takes_the_rest_only_at_the_end() {
        let eps = tvdb(&[(1, 12, "2020-01-05"), (2, 5, "2026-10-04")]);
        let entries = [
            entry(1, 1, Some(12), "2020-01-05"),
            entry(2, 2, None, "2026-10-04"),
        ];
        let m = map_episodes(&eps, &entries, false).unwrap();
        assert_eq!(spans(&m), vec![(0, 0, 12), (1, 12, 5)]);
        assert!(map_episodes(&eps, &entries, true).is_err());
    }

    #[test]
    fn only_followed_seasons_become_members() {
        let mut eps = tvdb(&[
            (1, 2, "2020-01-05"),
            (2, 2, "2021-01-05"),
            (3, 2, "2022-01-05"),
        ]);
        eps[3].file_id = Some(1); // season 2, with a file
        eps[4].monitored = true; // season 3, monitored
        let entries = [
            entry(1, 1, Some(2), "2020-01-05"),
            entry(2, 2, Some(2), "2021-01-05"),
            entry(3, 3, Some(2), "2022-01-05"),
        ];
        let m = map_episodes(&eps, &entries, false).unwrap();
        let follow: Vec<bool> = m.chunks.iter().map(|c| followed(&eps, c)).collect();
        assert_eq!(follow, vec![false, true, true]);
    }

    #[test]
    fn files_move_with_their_companions_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let group = dir.path().join("Show");
        let s1 = group.join("Season 01");
        std::fs::create_dir_all(&s1).unwrap();
        for n in [
            "a - S01E13.mkv",
            "a - S01E13.en.srt",
            "a - S01E01.mkv",
            "other.txt",
        ] {
            std::fs::write(s1.join(n), b"x").unwrap();
        }
        let files = vec![
            (s1.join("a - S01E13.mkv"), group.join("Season 02")),
            (s1.join("a - S01E01.mkv"), s1.clone()),
        ];
        let plan = plan_moves(&files);
        assert!(plan.conflicts.is_empty());
        let mut targets: Vec<String> = plan
            .moves
            .iter()
            .map(|(_, t)| t.strip_prefix(&group).unwrap().display().to_string())
            .collect();
        targets.sort();
        assert_eq!(
            targets,
            vec!["Season 02/a - S01E13.en.srt", "Season 02/a - S01E13.mkv"]
        );
        perform_moves(&plan.moves).unwrap();
        assert!(group.join("Season 02/a - S01E13.mkv").is_file());
        assert!(!s1.join("a - S01E13.en.srt").exists());
        // An existing destination is a conflict.
        std::fs::write(s1.join("b.mkv"), b"x").unwrap();
        std::fs::write(group.join("Season 02/b.mkv"), b"y").unwrap();
        let plan = plan_moves(&[(s1.join("b.mkv"), group.join("Season 02"))]);
        assert_eq!(plan.conflicts.len(), 1);
    }
}
