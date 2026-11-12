//! The importer.
//!
//! One code path serves both modes:
//! * **dry run** — everything is written inside a transaction which is then rolled back, so
//!   the preview reports exactly what applying would do (including constraint violations);
//! * **apply** — a `VACUUM INTO` backup of the Kitsunarr database is taken first, the same
//!   transaction is committed only if every validation check passes.
//!
//! Re-running a migration is idempotent: every created record is recorded in
//! `migration_entity_map` keyed by the source fingerprint, and natural keys (provider identity,
//! names, paths) are matched as a fallback. Files on disk are never created, moved or deleted.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use kitsunarr_db::library::collections::{self, Member, NewCollection};
use kitsunarr_db::library::movies::{self, NewMovie, NewMovieFile};
use kitsunarr_db::library::series::{self, NewEpisode, NewEpisodeFile, NewSeason, NewSeries};
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_db::{
    Database, history, maintenance, migration_runs, quality, root_folders, rules, settings, tags,
};
use kitsunarr_domain::api::migration::{
    ConflictPolicy, EntityPlanSummary, MigrationOptions, MigrationWarning, PathPreview,
    ValidationCheck, WarningSeverity,
};
use kitsunarr_domain::api::settings::{
    CustomFormat, DownloadClientOptionsSettings, FormatScore, ImportListOptionsSettings,
    IndexerOptionsSettings, LibraryToolsSettings, ListSyncLevel, MediaManagementSettings,
    MetadataSourcesSettings, NamingSettings, OrderedF64, ProperDownloadType, ProviderKind,
    QualityDefinition, QualityGroup, QualityProfile, QualityProfileItem, QualitySizeLimit,
};
use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaKind, MetadataProvider, MinimumAvailability, MovieStatus,
    Quality, SeriesStatus, SeriesType,
};
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use crate::model::*;
use crate::paths::PathMapper;
use crate::{LoadedSource, MigrateError};

/// Media-management keys (lower-case source names) that Kitsunarr maps.
pub const MAPPED_MEDIA_MANAGEMENT_KEYS: &[&str] = &[
    "copyusinghardlinks",
    "recyclebin",
    "recyclebincleanupdays",
    "deleteemptyfolders",
    "skipunmonitoredepisodesfrompacks",
    "createemptyseriesfolders",
    "minimumfreespacewhenimporting",
    "setpermissionslinux",
    "chmodfolder",
    "skipfreespacecheckwhenimporting",
    "importextrafiles",
    "extrafileextensions",
    "downloadpropersandrepacks",
    // Indexer options (database `Config` keys; the API serves them separately).
    "minimumage",
    "retention",
    "maximumsize",
    "rsssyncinterval",
    "enablemultiseasonreleases",
    "enablefakereleaseprotection",
    // Download client options (database `Config` keys).
    "enablecompleteddownloadhandling",
    "autoredownloadfailed",
    "autoredownloadfailedfrominteractivesearch",
    "stalledtorrenttimeout",
    // Library tools (Tanukarr's `ProOptions` JSON; per-series options are not mapped).
    "prooptions",
    "proaccesskeys",
    // Import list options (database `Config` keys).
    "listsynclevel",
    "listsynctag",
    // Catalogue keys (Tanukarr).
    "tmdbapikey",
    "omdbapikey",
];

/// Custom-format specification implementations Sonarr v4 provides (others are kept but flagged).
const KNOWN_SPECS: &[&str] = &[
    "ReleaseTitleSpecification",
    "LanguageSpecification",
    "IndexerFlagSpecification",
    "SourceSpecification",
    "ResolutionSpecification",
    "SizeSpecification",
    "ReleaseGroupSpecification",
    "ReleaseTypeSpecification",
    "SeasonPackSpecification",
];

const MAX_WARNINGS: usize = 2000;
const MAX_PATH_PREVIEWS: usize = 500;

#[derive(Debug, Clone)]
pub enum Mode {
    DryRun,
    Apply { backup_dir: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub summary: Vec<EntityPlanSummary>,
    pub warnings: Vec<MigrationWarning>,
    pub paths: Vec<PathPreview>,
    pub blocking: Vec<String>,
    pub validation: Vec<ValidationCheck>,
    pub run_id: Option<i64>,
    pub backup_path: Option<String>,
}

pub type Progress<'a> = &'a (dyn Fn(&str, u64, u64) + Send + Sync);

#[derive(Clone, Copy)]
enum Act {
    Create,
    Update,
    Skip,
    Unsupported,
}

const ORDER: &[&str] = &[
    "tags",
    "rootFolders",
    "qualityDefinitions",
    "customFormats",
    "qualityProfiles",
    "downloadClients",
    "indexers",
    "notifications",
    "importLists",
    "delayProfiles",
    "releaseProfiles",
    "remotePathMappings",
    "importListExclusions",
    "settings",
    "series",
    "seasons",
    "episodeFiles",
    "episodes",
    "movies",
    "movieFiles",
    "collections",
    "history",
];

/// Collection title, kind and its members `(member, sort key, role, label)`.
type GroupPlan = (
    String,
    &'static str,
    Vec<(Member, i64, &'static str, Option<String>)>,
);

/// Aggregated per-series counts: (episodes, files, monitored, linked).
type SeriesCounts = (i64, i64, i64, i64);

/// Per written series: expected counts for validation.
struct Expect {
    target: i64,
    episodes: i64,
    files: i64,
    monitored: i64,
    linked: i64,
    path: String,
}

struct Importer<'a> {
    snap: &'a SourceSnapshot,
    fp: &'a str,
    run_id: i64,
    opts: &'a MigrationOptions,
    mapper: PathMapper,
    exists: HashMap<String, bool>,
    groups: HashSet<String>,
    counts: BTreeMap<&'static str, EntityPlanSummary>,
    warnings: Vec<MigrationWarning>,
    dropped_warnings: usize,
    paths: Vec<PathPreview>,
    tag_map: HashMap<i64, i64>,
    cf_map: HashMap<i64, i64>,
    profile_map: HashMap<i64, i64>,
    client_map: HashMap<i64, i64>,
    indexer_map: HashMap<i64, i64>,
    series_map: HashMap<i64, i64>,
    episode_map: HashMap<i64, i64>,
    movie_by_series: HashMap<i64, i64>,
    default_profile: i64,
    unmapped: Vec<(String, Option<String>, String, Value)>,
    expects: Vec<Expect>,
    movies_written: Vec<i64>,
    progress: Progress<'a>,
}

impl<'a> Importer<'a> {
    fn enabled(&self, group: &str) -> bool {
        self.groups.is_empty() || self.groups.contains(group)
    }

    fn count(&mut self, entity: &'static str, act: Act) {
        let e = self
            .counts
            .entry(entity)
            .or_insert_with(|| EntityPlanSummary {
                entity: entity.into(),
                ..Default::default()
            });
        match act {
            Act::Create => e.create += 1,
            Act::Update => e.update += 1,
            Act::Skip => e.skip += 1,
            Act::Unsupported => e.unsupported += 1,
        }
    }

    fn warn(
        &mut self,
        severity: WarningSeverity,
        entity: &str,
        source_id: Option<i64>,
        title: Option<&str>,
        message: impl Into<String>,
    ) {
        if self.warnings.len() >= MAX_WARNINGS {
            self.dropped_warnings += 1;
            return;
        }
        self.warnings.push(MigrationWarning {
            severity,
            entity: entity.into(),
            source_id: source_id.map(|i| i.to_string()),
            title: title.map(Into::into),
            message: message.into(),
        });
    }

    fn keep_unmapped(&mut self, entity: &str, source_id: Option<i64>, field: &str, value: Value) {
        self.unmapped.push((
            entity.into(),
            source_id.map(|i| i.to_string()),
            field.into(),
            value,
        ));
    }

    fn update_existing(&self) -> bool {
        self.opts.conflict_policy == ConflictPolicy::Update
    }

    fn map_tags(&self, source: &[i64]) -> Vec<i64> {
        source
            .iter()
            .filter_map(|t| self.tag_map.get(t).copied())
            .collect()
    }

    /// Target id from the migration map, if the record still exists.
    async fn mapped(
        &self,
        conn: &mut SqliteConnection,
        entity: &str,
        source_id: i64,
        table: &'static str,
    ) -> Result<Option<i64>, MigrateError> {
        let Some(id) =
            migration_runs::lookup(conn, self.fp, entity, &source_id.to_string()).await?
        else {
            return Ok(None);
        };
        let sql = format!("SELECT id FROM {table} WHERE id = ?");
        let exists: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
        Ok(exists)
    }

    async fn remember(
        &self,
        conn: &mut SqliteConnection,
        entity: &str,
        source_id: i64,
        target: i64,
    ) -> Result<(), MigrateError> {
        migration_runs::record(
            conn,
            self.fp,
            entity,
            &source_id.to_string(),
            target,
            self.run_id,
        )
        .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------------------------

    async fn tags(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        // Tags are always resolved because other entities reference them.
        for (sid, label) in &self.snap.tags {
            let normalized = match tags::normalize_label(label) {
                Ok(l) => l,
                Err(_) => {
                    self.warn(
                        WarningSeverity::Warning,
                        "tags",
                        Some(*sid),
                        Some(label),
                        "Tag label is invalid and was skipped.",
                    );
                    self.count("tags", Act::Unsupported);
                    continue;
                }
            };
            let existing: Option<i64> = match self.mapped(conn, "tag", *sid, "tags").await? {
                Some(id) => Some(id),
                None => {
                    sqlx::query_scalar("SELECT id FROM tags WHERE label = ?")
                        .bind(&normalized)
                        .fetch_optional(&mut *conn)
                        .await?
                }
            };
            let id = match existing {
                Some(id) => {
                    self.count("tags", Act::Skip);
                    id
                }
                None if self.enabled("tags") => {
                    self.count("tags", Act::Create);
                    tags::create(&mut *conn, &normalized).await?
                }
                None => continue,
            };
            self.tag_map.insert(*sid, id);
            self.remember(conn, "tag", *sid, id).await?;
        }
        Ok(())
    }

    async fn root_folders(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        if !self.enabled("rootFolders") {
            return Ok(());
        }
        for path in self.snap.root_paths() {
            let target = root_folders::normalize_path(&self.mapper.map(&path));
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT id FROM root_folders WHERE path = ?")
                    .bind(&target)
                    .fetch_optional(&mut *conn)
                    .await?;
            if exists.is_some() {
                self.count("rootFolders", Act::Skip);
                continue;
            }
            let under: Vec<&SrcSeries> = self
                .snap
                .series
                .iter()
                .filter(|s| s.root_folder_path.as_deref() == Some(path.as_str()))
                .collect();
            let kind = if !under.is_empty()
                && under.iter().all(|s| s.media_type == SrcMediaType::Movie)
            {
                Some(MediaKind::Movie)
            } else if !under.is_empty() && under.iter().all(|s| s.series_type == SeriesType::Anime)
            {
                Some(MediaKind::Anime)
            } else {
                None
            };
            root_folders::create(&mut *conn, &target, kind).await?;
            self.count("rootFolders", Act::Create);
            if self.exists.get(&target) == Some(&false) {
                self.warn(WarningSeverity::Warning, "rootFolders", None, Some(&target), format!(
                    "Root folder {target} (source {path}) does not exist for Kitsunarr. Check the path mappings or the container volumes."
                ));
            }
        }
        Ok(())
    }

    async fn quality_definitions(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<(), MigrateError> {
        if !self.enabled("qualityDefinitions") {
            return Ok(());
        }
        let current = quality::list_definitions(&mut *conn).await?;
        let defaults = quality::default_definitions();
        for d in &self.snap.quality_definitions {
            if d.min_size.is_none() && d.max_size.is_none() && d.preferred_size.is_none() {
                continue; // Tanukarr keeps sizes on profile items instead.
            }
            let Some(q) = Quality::by_id(d.quality_id) else {
                self.warn(WarningSeverity::Info, "qualityDefinitions", Some(i64::from(d.quality_id)), Some(&d.title),
                    "This quality does not exist in Kitsunarr's catalogue; its size limits were not imported.");
                self.count("qualityDefinitions", Act::Unsupported);
                continue;
            };
            let cur = current.iter().find(|c| c.quality_id == q.id);
            let is_default = match (cur, defaults.iter().find(|c| c.quality_id == q.id)) {
                (Some(c), Some(def)) => {
                    c.min_size == def.min_size
                        && c.max_size == def.max_size
                        && c.preferred_size == def.preferred_size
                }
                _ => true,
            };
            let new = QualityDefinition {
                quality_id: q.id,
                title: cur.map_or_else(|| q.name.to_string(), |c| c.title.clone()),
                weight: q.weight,
                min_size: d.min_size.map(OrderedF64),
                preferred_size: d.preferred_size.map(OrderedF64),
                max_size: d.max_size.map(OrderedF64),
            };
            if cur.is_some_and(|c| {
                c.min_size == new.min_size
                    && c.max_size == new.max_size
                    && c.preferred_size == new.preferred_size
            }) {
                self.count("qualityDefinitions", Act::Skip);
            } else if is_default || self.update_existing() {
                quality::upsert_definition(&mut *conn, &new).await?;
                self.count("qualityDefinitions", Act::Update);
            } else {
                self.count("qualityDefinitions", Act::Skip);
            }
        }
        Ok(())
    }

    async fn custom_formats(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        let existing = quality::list_custom_formats(&mut *conn).await?;
        for cf in &self.snap.custom_formats {
            for spec in &cf.specifications {
                if !KNOWN_SPECS.contains(&spec.implementation.as_str()) {
                    self.warn(
                        WarningSeverity::Info,
                        "customFormats",
                        Some(cf.id),
                        Some(&cf.name),
                        format!(
                            "Specification type {} is kept but not evaluated by Kitsunarr yet.",
                            spec.implementation
                        ),
                    );
                }
            }
            let model = CustomFormat {
                id: 0,
                name: cf.name.clone(),
                include_when_renaming: cf.include_when_renaming,
                specifications: cf.specifications.clone(),
            };
            let found = match self
                .mapped(conn, "customFormat", cf.id, "custom_formats")
                .await?
            {
                Some(id) => Some(id),
                None => existing
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(&cf.name))
                    .map(|e| e.id),
            };
            let target = match found {
                Some(id) if self.update_existing() && self.enabled("customFormats") => {
                    quality::update_custom_format(&mut *conn, &CustomFormat { id, ..model })
                        .await?;
                    self.count("customFormats", Act::Update);
                    id
                }
                Some(id) => {
                    self.count("customFormats", Act::Skip);
                    id
                }
                None if self.enabled("customFormats") => {
                    match quality::create_custom_format(&mut *conn, &model).await {
                        Ok(id) => {
                            self.count("customFormats", Act::Create);
                            id
                        }
                        Err(e) => {
                            self.warn(
                                WarningSeverity::Warning,
                                "customFormats",
                                Some(cf.id),
                                Some(&cf.name),
                                format!("Not imported: {e}."),
                            );
                            self.count("customFormats", Act::Unsupported);
                            continue;
                        }
                    }
                }
                None => continue,
            };
            self.cf_map.insert(cf.id, target);
            self.remember(conn, "customFormat", cf.id, target).await?;
        }
        Ok(())
    }

    fn convert_profile(&mut self, p: &SrcQualityProfile) -> QualityProfile {
        let mut items = Vec::new();
        let mut dropped = Vec::new();
        for item in &p.items {
            match (&item.quality_id, &item.group) {
                (Some(q), _) => {
                    if Quality::by_id(*q).is_some() {
                        items.push(QualityProfileItem {
                            quality_id: Some(*q),
                            group: None,
                            allowed: item.allowed,
                        });
                    } else {
                        dropped.push(*q);
                    }
                }
                (None, Some((gid, name, members))) => {
                    let known: Vec<i32> = members
                        .iter()
                        .copied()
                        .filter(|q| Quality::by_id(*q).is_some())
                        .collect();
                    dropped.extend(
                        members
                            .iter()
                            .copied()
                            .filter(|q| Quality::by_id(*q).is_none()),
                    );
                    if !known.is_empty() {
                        items.push(QualityProfileItem {
                            quality_id: None,
                            group: Some(QualityGroup {
                                id: *gid,
                                name: name.clone(),
                                quality_ids: known,
                            }),
                            allowed: item.allowed,
                        });
                    }
                }
                _ => {}
            }
        }
        // Qualities absent from the source profile are appended as not allowed so every
        // catalogue quality is rankable (Sonarr adds new qualities the same way).
        let present: HashSet<i32> = items
            .iter()
            .flat_map(|i| {
                i.quality_id
                    .into_iter()
                    .chain(i.group.iter().flat_map(|g| g.quality_ids.clone()))
            })
            .collect();
        for q in Quality::all() {
            if !present.contains(&q.id) {
                items.push(QualityProfileItem {
                    quality_id: Some(q.id),
                    group: None,
                    allowed: false,
                });
            }
        }
        if !dropped.is_empty() {
            self.warn(WarningSeverity::Warning, "qualityProfiles", Some(p.id), Some(&p.name), format!(
                "Qualities {dropped:?} are not in Kitsunarr's catalogue (custom presets) and were removed from the profile."
            ));
            self.keep_unmapped(
                "qualityProfile",
                Some(p.id),
                "droppedQualities",
                json!(dropped),
            );
        }
        let cutoff_ok = items.iter().any(|i| {
            i.allowed
                && (i.quality_id == Some(p.cutoff)
                    || i.group.as_ref().is_some_and(|g| g.id == p.cutoff))
        });
        let cutoff =
            if cutoff_ok {
                p.cutoff
            } else {
                let fallback = items
                    .iter()
                    .rev()
                    .find(|i| i.allowed)
                    .map(|i| {
                        i.quality_id
                            .unwrap_or_else(|| i.group.as_ref().map_or(0, |g| g.id))
                    })
                    .unwrap_or(0);
                self.warn(WarningSeverity::Warning, "qualityProfiles", Some(p.id), Some(&p.name),
                "The cutoff quality is not available; the best allowed quality is used instead.");
                fallback
            };
        let mut format_items = Vec::new();
        for (format, score) in &p.format_items {
            match self.cf_map.get(format) {
                Some(id) => format_items.push(FormatScore {
                    format_id: *id,
                    score: *score,
                }),
                None if *score != 0 => {
                    self.warn(WarningSeverity::Warning, "qualityProfiles", Some(p.id), Some(&p.name), format!(
                        "Score {score} for custom format #{format} was dropped because that format was not imported."
                    ));
                }
                None => {}
            }
        }
        for (k, v) in &p.unmapped {
            self.keep_unmapped("qualityProfile", Some(p.id), k, v.clone());
        }
        if !p.unmapped.is_empty() {
            self.warn(WarningSeverity::Info, "qualityProfiles", Some(p.id), Some(&p.name), format!(
                "Tanukarr-specific profile settings were preserved in the migration report but are not applied: {}.",
                p.unmapped.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let mut size_limits = Vec::new();
        for l in &p.size_limits {
            if Quality::by_id(l.quality_id).is_none() {
                self.warn(WarningSeverity::Warning, "qualityProfiles", Some(p.id), Some(&p.name), format!(
                    "Size limits of quality #{} were dropped: Kitsunarr has no such quality (custom presets are not imported).",
                    l.quality_id
                ));
                continue;
            }
            let o = |v: Option<f64>| {
                v.filter(|x| x.is_finite())
                    .map(|x| OrderedF64(x.clamp(0.0, 1000.0)))
            };
            size_limits.push(QualitySizeLimit {
                quality_id: l.quality_id,
                min_size: o(l.min_size),
                preferred_size: o(l.preferred_size),
                max_size: o(l.max_size),
            });
        }
        let mut codec_multipliers = std::collections::BTreeMap::new();
        for (k, v) in &p.codec_multipliers {
            match kitsunarr_domain::codec::canonical(k) {
                Some(family) if (0.01..=10.0).contains(v) => {
                    codec_multipliers.insert(family.to_string(), OrderedF64(*v));
                }
                _ => self.warn(WarningSeverity::Warning, "qualityProfiles", Some(p.id), Some(&p.name), format!(
                    "The codec factor {k} = {v} was dropped: unknown codec or outside 0.01 to 10."
                )),
            }
        }
        let profile = QualityProfile {
            id: 0,
            name: p.name.clone(),
            upgrade_allowed: p.upgrade_allowed,
            cutoff,
            items,
            min_format_score: p.min_format_score,
            cutoff_format_score: p.cutoff_format_score,
            min_upgrade_format_score: p.min_upgrade_format_score.max(1),
            format_items,
            size_limits,
            codec_multipliers,
        };
        if quality::validate_profile(&profile).is_err() {
            // Inconsistent source sizes (minimum above maximum): keep the profile without them.
            let mut fallback = profile.clone();
            fallback.size_limits.clear();
            if quality::validate_profile(&fallback).is_ok() {
                self.warn(WarningSeverity::Warning, "qualityProfiles", Some(p.id), Some(&p.name),
                    "The profile's size limits are inconsistent and were dropped; the global quality definitions apply.".to_string());
                return fallback;
            }
        }
        profile
    }

    async fn quality_profiles(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        let existing = quality::list_profiles(&mut *conn).await?;
        self.default_profile = existing.first().map_or(0, |p| p.id);
        for p in &self.snap.quality_profiles {
            let model = self.convert_profile(p);
            let found = match self
                .mapped(conn, "qualityProfile", p.id, "quality_profiles")
                .await?
            {
                Some(id) => Some(id),
                None => existing
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(&p.name))
                    .map(|e| e.id),
            };
            let target = match found {
                Some(id) => {
                    let current = existing.iter().find(|e| e.id == id);
                    // Kitsunarr's untouched built-in profiles take the source definition.
                    let pristine = current.is_some_and(|c| {
                        quality::default_profiles().iter().any(|d| {
                            d.name == c.name
                                && d.items == c.items
                                && d.cutoff == c.cutoff
                                && c.format_items.is_empty()
                        })
                    });
                    if (pristine || self.update_existing()) && self.enabled("qualityProfiles") {
                        match quality::update_profile(&mut *conn, &QualityProfile { id, ..model })
                            .await
                        {
                            Ok(()) => self.count("qualityProfiles", Act::Update),
                            Err(e) => {
                                self.warn(
                                    WarningSeverity::Warning,
                                    "qualityProfiles",
                                    Some(p.id),
                                    Some(&p.name),
                                    format!("Not updated: {e}."),
                                );
                                self.count("qualityProfiles", Act::Skip);
                            }
                        }
                    } else {
                        self.count("qualityProfiles", Act::Skip);
                    }
                    id
                }
                None if self.enabled("qualityProfiles") => {
                    match quality::create_profile(&mut *conn, &model).await {
                        Ok(id) => {
                            self.count("qualityProfiles", Act::Create);
                            id
                        }
                        Err(e) => {
                            self.warn(
                                WarningSeverity::Warning,
                                "qualityProfiles",
                                Some(p.id),
                                Some(&p.name),
                                format!("Not imported: {e}."),
                            );
                            self.count("qualityProfiles", Act::Unsupported);
                            continue;
                        }
                    }
                }
                None => continue,
            };
            self.profile_map.insert(p.id, target);
            self.remember(conn, "qualityProfile", p.id, target).await?;
        }
        Ok(())
    }

    async fn providers(
        &mut self,
        conn: &mut SqliteConnection,
        kind: ProviderKind,
        group: &'static str,
    ) -> Result<(), MigrateError> {
        if !self.enabled(group) {
            return Ok(());
        }
        let entity = match kind {
            ProviderKind::Indexer => "indexer",
            ProviderKind::DownloadClient => "downloadClient",
            ProviderKind::Notification => "notification",
            ProviderKind::ImportList => "importList",
        };
        let existing = providers::list(&mut *conn, kind).await?;
        let sources: Vec<&SrcProvider> = self
            .snap
            .providers
            .iter()
            .filter(|p| p.kind == kind)
            .collect();
        for p in sources {
            let mut options = p.options.clone();
            if kind == ProviderKind::Indexer
                && let Some(cid) = options.get("downloadClientId").and_then(Value::as_i64)
            {
                options.insert(
                    "downloadClientId".into(),
                    self.client_map.get(&cid).map_or(Value::Null, |v| json!(v)),
                );
            }
            if kind == ProviderKind::ImportList {
                if let Some(root) = options
                    .get("rootFolderPath")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                {
                    options.insert("rootFolderPath".into(), json!(self.mapper.map(&root)));
                }
                if let Some(pid) = options.get("qualityProfileId").and_then(Value::as_i64) {
                    options.insert(
                        "qualityProfileId".into(),
                        self.profile_map.get(&pid).map_or(Value::Null, |v| json!(v)),
                    );
                }
            }
            let record = ProviderRecord {
                id: 0,
                kind,
                name: p.name.clone(),
                implementation: p.implementation.clone(),
                protocol: providers::protocol_for(&p.implementation),
                enabled: p.enabled,
                priority: p.priority,
                tags: self.map_tags(&p.tags),
                options,
                settings: p.settings.clone(),
            };
            let found = match self
                .mapped(conn, entity, p.id, provider_table(kind))
                .await?
            {
                Some(id) => Some(id),
                None => existing
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(&p.name))
                    .map(|e| e.id),
            };
            let target = match found {
                Some(id) if self.update_existing() => {
                    let mut rec = ProviderRecord { id, ..record };
                    // Never overwrite a stored secret with a masked (empty) one.
                    if let Some(current) = existing.iter().find(|e| e.id == id) {
                        for field in &p.masked_fields {
                            if let Some(v) = current.settings.get(field) {
                                rec.settings.insert(field.clone(), v.clone());
                            }
                        }
                    }
                    providers::update(&mut *conn, &rec).await?;
                    self.count(group, Act::Update);
                    id
                }
                Some(id) => {
                    self.count(group, Act::Skip);
                    id
                }
                None => {
                    let id = providers::create(&mut *conn, &record).await?;
                    self.count(group, Act::Create);
                    if !p.masked_fields.is_empty() {
                        self.warn(WarningSeverity::Warning, group, Some(p.id), Some(&p.name), format!(
                            "The source API does not reveal secrets; re-enter: {}. (Backup migrations include them.)",
                            p.masked_fields.join(", ")
                        ));
                    }
                    if !providers::is_supported(kind, &p.implementation) {
                        self.warn(WarningSeverity::Info, group, Some(p.id), Some(&p.name), format!(
                            "{} is stored with all its settings but is not implemented natively yet.", p.implementation
                        ));
                    }
                    id
                }
            };
            match kind {
                ProviderKind::Indexer => self.indexer_map.insert(p.id, target),
                ProviderKind::DownloadClient => self.client_map.insert(p.id, target),
                _ => None,
            };
            self.remember(conn, entity, p.id, target).await?;
        }
        Ok(())
    }

    async fn delay_and_release_profiles(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<(), MigrateError> {
        if self.enabled("delayProfiles") {
            let existing = rules::list_delay_profiles(&mut *conn).await?;
            for d in &self.snap.delay_profiles {
                let tags = self.map_tags(&d.tags);
                let order = d.order.min(i64::from(i32::MAX));
                let found = match self
                    .mapped(conn, "delayProfile", d.id, "delay_profiles")
                    .await?
                {
                    Some(id) => Some(id),
                    None => existing
                        .iter()
                        .find(|e| e.sort_order == order && e.tags == tags)
                        .map(|e| e.id),
                };
                let target = match found {
                    Some(id) => {
                        self.count("delayProfiles", Act::Skip);
                        id
                    }
                    None => {
                        self.count("delayProfiles", Act::Create);
                        rules::create_delay_profile(
                            &mut *conn,
                            &rules::DelayProfile {
                                id: 0,
                                enable_usenet: d.enable_usenet,
                                enable_torrent: d.enable_torrent,
                                preferred_protocol: d.preferred_protocol.clone(),
                                usenet_delay: d.usenet_delay,
                                torrent_delay: d.torrent_delay,
                                bypass_if_highest_quality: d.bypass_if_highest_quality,
                                bypass_if_above_custom_format_score: d
                                    .bypass_if_above_custom_format_score,
                                minimum_custom_format_score: d.minimum_custom_format_score,
                                sort_order: order,
                                tags,
                            },
                        )
                        .await?
                    }
                };
                self.remember(conn, "delayProfile", d.id, target).await?;
            }
        }
        if self.enabled("releaseProfiles") {
            for r in &self.snap.release_profiles {
                for (k, v) in &r.unmapped {
                    self.keep_unmapped("releaseProfile", Some(r.id), k, v.clone());
                }
                if let Some(id) = self
                    .mapped(conn, "releaseProfile", r.id, "release_profiles")
                    .await?
                {
                    self.count("releaseProfiles", Act::Skip);
                    self.remember(conn, "releaseProfile", r.id, id).await?;
                    continue;
                }
                let indexer_ids: Vec<i64> = r
                    .indexer_ids
                    .iter()
                    .filter_map(|i| self.indexer_map.get(i).copied())
                    .collect();
                if indexer_ids.len() != r.indexer_ids.len() {
                    self.warn(WarningSeverity::Warning, "releaseProfiles", Some(r.id), r.name.as_deref(),
                        "Some indexers this profile was limited to were not imported; it now applies to the remaining ones (or all).");
                }
                let id = rules::create_release_profile(
                    &mut *conn,
                    &rules::ReleaseProfile {
                        id: 0,
                        name: r.name.clone(),
                        enabled: r.enabled,
                        required: r.required.clone(),
                        ignored: r.ignored.clone(),
                        indexer_ids,
                        tags: self.map_tags(&r.tags),
                    },
                )
                .await?;
                self.count("releaseProfiles", Act::Create);
                self.remember(conn, "releaseProfile", r.id, id).await?;
            }
        }
        if self.enabled("remotePathMappings") {
            let existing = rules::list_remote_path_mappings(&mut *conn).await?;
            for (_, host, remote, local) in &self.snap.remote_path_mappings {
                if existing
                    .iter()
                    .any(|e| e.host.eq_ignore_ascii_case(host) && e.remote_path == *remote)
                {
                    self.count("remotePathMappings", Act::Skip);
                    continue;
                }
                rules::create_remote_path_mapping(
                    &mut *conn,
                    host,
                    remote,
                    &self.mapper.map(local),
                )
                .await?;
                self.count("remotePathMappings", Act::Create);
            }
        }
        if self.enabled("importListExclusions") {
            for e in &self.snap.exclusions {
                let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM import_list_exclusions")
                    .fetch_one(&mut *conn)
                    .await?;
                rules::upsert_exclusion(
                    conn,
                    e.provider.as_str(),
                    &e.external_id,
                    e.title.as_deref(),
                )
                .await?;
                let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM import_list_exclusions")
                    .fetch_one(&mut *conn)
                    .await?;
                self.count(
                    "importListExclusions",
                    if after > before {
                        Act::Create
                    } else {
                        Act::Skip
                    },
                );
            }
        }
        Ok(())
    }

    async fn settings(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        if !self.enabled("settings") {
            return Ok(());
        }
        if let Some(n) = &self.snap.naming {
            let current: NamingSettings =
                settings::get_or_default(&mut *conn, settings::KEY_NAMING).await?;
            let mut next = current.clone();
            next.rename_episodes = n.bool_or("renameepisodes", next.rename_episodes);
            next.replace_illegal_characters =
                n.bool_or("replaceillegalcharacters", next.replace_illegal_characters);
            next.colon_replacement_format = n
                .i32("colonreplacementformat")
                .unwrap_or(next.colon_replacement_format);
            next.custom_colon_replacement_format =
                n.str("customcolonreplacementformat").unwrap_or_default();
            next.multi_episode_style = n
                .i32("multiepisodestyle")
                .unwrap_or(next.multi_episode_style);
            for (key, slot) in [
                ("standardepisodeformat", &mut next.standard_episode_format),
                ("dailyepisodeformat", &mut next.daily_episode_format),
                ("animeepisodeformat", &mut next.anime_episode_format),
                ("seriesfolderformat", &mut next.series_folder_format),
                ("seasonfolderformat", &mut next.season_folder_format),
                ("specialsfolderformat", &mut next.specials_folder_format),
                ("movieformat", &mut next.movie_file_format),
            ] {
                if let Some(v) = n.str(key) {
                    *slot = v;
                }
            }
            for key in [
                "animegroupfolderformat",
                "animegroupseasonfolderformat",
                "animegrouparcfolderformat",
                "animegroupmoviefolderformat",
                "animegroupmoviesagafolderformat",
            ] {
                if let Some(v) = n.str(key) {
                    self.keep_unmapped("naming", None, key, json!(v));
                }
            }
            if next == current {
                self.count("settings", Act::Skip);
            } else if current == NamingSettings::default() || self.update_existing() {
                settings::set(&mut *conn, settings::KEY_NAMING, &next).await?;
                self.count("settings", Act::Update);
            } else {
                self.count("settings", Act::Skip);
                self.warn(
                    WarningSeverity::Info,
                    "settings",
                    None,
                    Some("Naming"),
                    "Kitsunarr's naming settings were already customised and were kept.",
                );
            }
        }
        if let Some(mm) = &self.snap.media_management {
            let current: MediaManagementSettings =
                settings::get_or_default(&mut *conn, settings::KEY_MEDIA_MANAGEMENT).await?;
            let mut next = current.clone();
            next.use_hardlinks = mm.bool_or("copyusinghardlinks", next.use_hardlinks);
            next.recycle_bin = mm
                .str("recyclebin")
                .map(|p| self.mapper.map(&p))
                .or(next.recycle_bin);
            next.recycle_bin_cleanup_days = mm
                .i32("recyclebincleanupdays")
                .unwrap_or(next.recycle_bin_cleanup_days);
            next.delete_empty_folders = mm.bool_or("deleteemptyfolders", next.delete_empty_folders);
            next.skip_unmonitored_episodes_from_packs = mm.bool_or(
                "skipunmonitoredepisodesfrompacks",
                next.skip_unmonitored_episodes_from_packs,
            );
            next.create_empty_series_folders =
                mm.bool_or("createemptyseriesfolders", next.create_empty_series_folders);
            next.minimum_free_space_mb = mm
                .i64("minimumfreespacewhenimporting")
                .unwrap_or(next.minimum_free_space_mb);
            next.set_permissions = mm.bool_or("setpermissionslinux", next.set_permissions);
            next.chmod_folder = mm
                .str("chmodfolder")
                .filter(|c| c.len() == 3)
                .unwrap_or(next.chmod_folder);
            next.skip_free_space_check = mm.bool_or(
                "skipfreespacecheckwhenimporting",
                next.skip_free_space_check,
            );
            next.import_extra_files = mm.bool_or("importextrafiles", next.import_extra_files);
            next.extra_file_extensions = mm
                .str("extrafileextensions")
                .unwrap_or(next.extra_file_extensions);
            if let Some(v) = mm.str("downloadpropersandrepacks") {
                match ProperDownloadType::from_sonarr(&v) {
                    Some(p) => next.download_propers_and_repacks = p,
                    None => self.warn(
                        WarningSeverity::Warning,
                        "settings",
                        None,
                        Some("Propers and repacks"),
                        format!("Unknown \"propers and repacks\" value {v:?}; Kitsunarr's setting was kept."),
                    ),
                }
            }
            if next == current {
                self.count("settings", Act::Skip);
            } else if current == MediaManagementSettings::default() || self.update_existing() {
                settings::set(&mut *conn, settings::KEY_MEDIA_MANAGEMENT, &next).await?;
                self.count("settings", Act::Update);
            } else {
                self.count("settings", Act::Skip);
            }
        }
        // Indexer options: the API's `/config/indexer`, or the database's `Config` keys.
        if let Some(io) = self
            .snap
            .indexer_options
            .clone()
            .or_else(|| self.snap.media_management.clone())
            .filter(|r| {
                [
                    "minimumage",
                    "retention",
                    "maximumsize",
                    "rsssyncinterval",
                    "enablemultiseasonreleases",
                    "enablefakereleaseprotection",
                ]
                .iter()
                .any(|k| r.has(k))
            })
        {
            let current: IndexerOptionsSettings =
                settings::get_or_default(&mut *conn, settings::KEY_INDEXER_OPTIONS).await?;
            let mut next = current.clone();
            next.minimum_age = io.i32("minimumage").unwrap_or(next.minimum_age);
            next.retention = io.i32("retention").unwrap_or(next.retention);
            next.maximum_size = io.i32("maximumsize").unwrap_or(next.maximum_size);
            next.rss_sync_interval = io.i32("rsssyncinterval").unwrap_or(next.rss_sync_interval);
            if let Some(v) = io.bool("enablemultiseasonreleases") {
                next.enable_multi_season_releases = v;
            }
            if let Some(v) = io.bool("enablefakereleaseprotection") {
                next.enable_fake_release_protection = v;
            }
            if next == current {
                self.count("settings", Act::Skip);
            } else if current == IndexerOptionsSettings::default() || self.update_existing() {
                settings::set(&mut *conn, settings::KEY_INDEXER_OPTIONS, &next).await?;
                self.count("settings", Act::Update);
            } else {
                self.count("settings", Act::Skip);
            }
        }
        // Download client options: the API's `/config/downloadclient`, or `Config` keys.
        if let Some(dc) = self
            .snap
            .download_client_options
            .clone()
            .or_else(|| self.snap.media_management.clone())
            .filter(|r| {
                [
                    "enablecompleteddownloadhandling",
                    "autoredownloadfailed",
                    "autoredownloadfailedfrominteractivesearch",
                    "stalledtorrenttimeout",
                ]
                .iter()
                .any(|k| r.has(k))
            })
        {
            let current: DownloadClientOptionsSettings =
                settings::get_or_default(&mut *conn, settings::KEY_DOWNLOAD_CLIENT_OPTIONS).await?;
            let mut next = current.clone();
            if let Some(v) = dc.bool("enablecompleteddownloadhandling") {
                next.enable_completed_download_handling = v;
            }
            if let Some(v) = dc.bool("autoredownloadfailed") {
                next.auto_redownload_failed = v;
            }
            if let Some(v) = dc.bool("autoredownloadfailedfrominteractivesearch") {
                next.auto_redownload_failed_from_interactive_search = v;
            }
            if let Some(v) = dc.i32("stalledtorrenttimeout") {
                next.stalled_torrent_timeout = v.clamp(0, 43_200);
            }
            if next == current {
                self.count("settings", Act::Skip);
            } else if current == DownloadClientOptionsSettings::default() || self.update_existing()
            {
                settings::set(&mut *conn, settings::KEY_DOWNLOAD_CLIENT_OPTIONS, &next).await?;
                self.count("settings", Act::Update);
            } else {
                self.count("settings", Act::Skip);
            }
        }
        // Tanukarr's Library tools switches (`ProOptions`, a JSON object in `Config`).
        if let Some(raw) = self
            .snap
            .media_management
            .clone()
            .and_then(|r| r.json("prooptions"))
            .filter(Value::is_object)
        {
            let flag = |k: &str| raw.get(k).and_then(Value::as_bool).unwrap_or(false);
            let next = LibraryToolsSettings {
                automation_paused: flag("automationPaused"),
                hardlink_only: flag("hardlinkOnly"),
                match_external_ids: flag("matchExternalIds"),
                prefer_anime_season_packs: flag("preferAnimeSeasonPacks"),
            };
            let current: LibraryToolsSettings =
                settings::get_or_default(&mut *conn, settings::KEY_LIBRARY_TOOLS).await?;
            if next == current {
                self.count("settings", Act::Skip);
            } else if current == LibraryToolsSettings::default() || self.update_existing() {
                settings::set(&mut *conn, settings::KEY_LIBRARY_TOOLS, &next).await?;
                self.count("settings", Act::Update);
            } else {
                self.count("settings", Act::Skip);
            }
        }
        // Tanukarr's companion app keys (`ProAccessKeys`): only hashes are stored, so the
        // tokens given to companion apps keep working.
        if let Some(Value::Array(keys)) = self
            .snap
            .media_management
            .clone()
            .and_then(|r| r.json("proaccesskeys"))
        {
            for k in keys {
                let field = |n: &str| {
                    k.get(n)
                        .or_else(|| k.get(format!("{}{}", n[..1].to_uppercase(), &n[1..])))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                };
                let (Some(id), Some(name), Some(hash), Some(permission), Some(expires)) = (
                    field("id"),
                    field("name"),
                    field("hash"),
                    field("permission"),
                    field("expires"),
                ) else {
                    continue;
                };
                if hash.len() != 64
                    || !hash.chars().all(|c| c.is_ascii_hexdigit())
                    || !matches!(permission.as_str(), "read-library" | "add-series")
                {
                    continue;
                }
                let expires = chrono::DateTime::parse_from_rfc3339(&expires)
                    .map(|d| {
                        d.with_timezone(&chrono::Utc)
                            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                    })
                    .unwrap_or(expires);
                sqlx::query(
                    "INSERT OR IGNORE INTO companion_keys (id, name, hash, permission, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(&name)
                .bind(hash.to_uppercase())
                .bind(&permission)
                .bind(field("created").unwrap_or_else(kitsunarr_db::now))
                .bind(&expires)
                .execute(&mut *conn)
                .await?;
                self.count("settings", Act::Create);
            }
        }
        // Import list options: the API's `/config/importlist`, or `Config` keys.
        if let Some(il) = self
            .snap
            .import_list_options
            .clone()
            .or_else(|| self.snap.media_management.clone())
            .filter(|r| r.has("listsynclevel"))
        {
            let current: ImportListOptionsSettings =
                settings::get_or_default(&mut *conn, settings::KEY_IMPORT_LIST_OPTIONS).await?;
            let mut next = current.clone();
            let level = match il.raw("listsynclevel") {
                Some(Value::String(s)) => s.to_ascii_lowercase(),
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(1) => "logonly".into(),
                    Some(2) => "keepandunmonitor".into(),
                    Some(3) => "keepandtag".into(),
                    _ => "disabled".into(),
                },
                _ => "disabled".into(),
            };
            next.list_sync_level = match level.as_str() {
                "logonly" => ListSyncLevel::LogOnly,
                "keepandunmonitor" => ListSyncLevel::KeepAndUnmonitor,
                "keepandtag" => ListSyncLevel::KeepAndTag,
                _ => ListSyncLevel::Disabled,
            };
            next.list_sync_tag = il
                .i64("listsynctag")
                .filter(|t| *t > 0)
                .and_then(|t| self.tag_map.get(&t).copied());
            if next.list_sync_level == ListSyncLevel::KeepAndTag && next.list_sync_tag.is_none() {
                next.list_sync_level = ListSyncLevel::Disabled;
            }
            if next == current {
                self.count("settings", Act::Skip);
            } else if current == ImportListOptionsSettings::default() || self.update_existing() {
                settings::set(&mut *conn, settings::KEY_IMPORT_LIST_OPTIONS, &next).await?;
                self.count("settings", Act::Update);
            } else {
                self.count("settings", Act::Skip);
            }
        }
        // Catalogue keys (Tanukarr's `tmdbapikey` and `omdbapikey` `Config` entries).
        if let Some(cfg) = self
            .snap
            .media_management
            .clone()
            .filter(|r| r.has("tmdbapikey") || r.has("omdbapikey"))
        {
            let current: MetadataSourcesSettings =
                settings::get_or_default(&mut *conn, settings::KEY_METADATA_SOURCES).await?;
            let mut next = current.clone();
            if let Some(k) = cfg.str("tmdbapikey").filter(|k| k != "********") {
                next.tmdb_api_key = Some(k);
            }
            if let Some(k) = cfg.str("omdbapikey").filter(|k| k != "********") {
                next.omdb_api_key = Some(k);
            }
            if next == current {
                self.count("settings", Act::Skip);
            } else if current == MetadataSourcesSettings::default() || self.update_existing() {
                settings::set(&mut *conn, settings::KEY_METADATA_SOURCES, &next).await?;
                self.count("settings", Act::Update);
            } else {
                self.count("settings", Act::Skip);
            }
        }
        for (k, v) in &self.snap.unmapped_settings {
            self.keep_unmapped("settings", None, k, v.clone());
        }
        if !self.snap.unmapped_settings.is_empty() {
            self.warn(WarningSeverity::Info, "settings", None, None, format!(
                "{} source settings have no Kitsunarr equivalent yet; their values are kept in the migration report.",
                self.snap.unmapped_settings.len()
            ));
        }
        Ok(())
    }

    fn external_ids(s: &SrcSeries, owner: (MetadataProvider, &str)) -> Vec<ExternalId> {
        let mut out = Vec::new();
        let mut push = |kind: ExternalIdKind, value: String| {
            if !value.is_empty()
                && value != "0"
                && !out
                    .iter()
                    .any(|e: &ExternalId| e.kind == kind && e.value == value)
            {
                out.push(ExternalId { kind, value });
            }
        };
        if s.tvdb_id > 0 {
            push(ExternalIdKind::Tvdb, s.tvdb_id.to_string());
        }
        if s.tv_rage_id > 0 {
            push(ExternalIdKind::TvRage, s.tv_rage_id.to_string());
        }
        if s.tv_maze_id > 0 {
            push(ExternalIdKind::TvMaze, s.tv_maze_id.to_string());
        }
        if s.tmdb_id > 0 {
            push(ExternalIdKind::Tmdb, s.tmdb_id.to_string());
        }
        if let Some(imdb) = &s.imdb_id {
            push(ExternalIdKind::Imdb, imdb.clone());
        }
        for id in &s.mal_ids {
            push(ExternalIdKind::MyAnimeList, id.to_string());
        }
        for id in &s.anilist_ids {
            push(ExternalIdKind::AniList, id.to_string());
        }
        let owner_kind = match owner.0 {
            MetadataProvider::Tvdb => Some(ExternalIdKind::Tvdb),
            MetadataProvider::TmdbTv | MetadataProvider::TmdbMovie => Some(ExternalIdKind::Tmdb),
            MetadataProvider::TvMaze => Some(ExternalIdKind::TvMaze),
            MetadataProvider::AniList => Some(ExternalIdKind::AniList),
            MetadataProvider::MyAnimeList => Some(ExternalIdKind::MyAnimeList),
            MetadataProvider::Omdb => Some(ExternalIdKind::Imdb),
        };
        if let Some(kind) = owner_kind {
            push(kind, owner.1.to_string());
        }
        out
    }

    fn profile_for(
        &mut self,
        entity: &str,
        source_id: i64,
        title: &str,
        source_profile: i64,
    ) -> i64 {
        match self.profile_map.get(&source_profile) {
            Some(id) => *id,
            None => {
                self.warn(
                    WarningSeverity::Warning,
                    entity,
                    Some(source_id),
                    Some(title),
                    "Its quality profile was not imported; Kitsunarr's first profile was assigned.",
                );
                self.default_profile
            }
        }
    }

    fn preview_path(&mut self, title: &str, source: &str, target: &str) {
        if self.paths.len() < MAX_PATH_PREVIEWS {
            self.paths.push(PathPreview {
                title: title.into(),
                source_path: source.into(),
                target_path: target.into(),
                exists: if self.opts.verify_paths {
                    self.exists.get(target).copied()
                } else {
                    None
                },
            });
        }
    }

    /// Tanukarr's per-series Library tools options (`ProOptions.series`): renaming mode and
    /// season folder titles ("keep versions" is not mapped).
    async fn series_naming_options(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<(), MigrateError> {
        let Some(Value::Object(per_series)) = self
            .snap
            .media_management
            .clone()
            .and_then(|r| r.json("prooptions"))
            .and_then(|v| v.get("series").cloned())
        else {
            return Ok(());
        };
        for (source_id, options) in per_series {
            let Some(&id) = source_id
                .parse::<i64>()
                .ok()
                .and_then(|s| self.series_map.get(&s))
            else {
                continue;
            };
            let mode = options
                .get("automaticRenaming")
                .and_then(Value::as_str)
                .filter(|m| matches!(*m, "default" | "manual" | "automatic"))
                .unwrap_or("default");
            let titles: std::collections::BTreeMap<i32, String> = options
                .get("seasonTitles")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| {
                            Some((k.parse().ok()?, v.as_str()?.chars().take(200).collect()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if mode == "default" && titles.is_empty() {
                continue;
            }
            sqlx::query("UPDATE series SET renaming_mode = ?, season_titles = ? WHERE id = ?")
                .bind(mode)
                .bind(serde_json::to_string(&titles).unwrap_or_else(|_| "{}".into()))
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }

    async fn series(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        if !self.enabled("series") {
            return Ok(());
        }
        let mut eps_by_series: HashMap<i64, Vec<&SrcEpisode>> = HashMap::new();
        for e in &self.snap.episodes {
            eps_by_series.entry(e.series_id).or_default().push(e);
        }
        let mut files_by_series: HashMap<i64, Vec<&SrcEpisodeFile>> = HashMap::new();
        for f in &self.snap.episode_files {
            files_by_series.entry(f.series_id).or_default().push(f);
        }
        let list: Vec<&SrcSeries> = self
            .snap
            .series
            .iter()
            .filter(|s| s.media_type != SrcMediaType::Movie)
            .collect();
        let total = list.len() as u64;
        for (i, s) in list.into_iter().enumerate() {
            if i % 25 == 0 {
                (self.progress)("series", i as u64, total);
            }
            let (provider, metadata_id) = s.identity();
            if metadata_id.is_empty() || metadata_id == "0" || metadata_id.starts_with('-') {
                self.warn(
                    WarningSeverity::Error,
                    "series",
                    Some(s.id),
                    Some(&s.title),
                    "The series has no usable metadata identifier and was skipped.",
                );
                self.count("series", Act::Unsupported);
                continue;
            }
            let path = self.mapper.map(&s.path);
            let root = self
                .mapper
                .map(s.root_folder_path.as_deref().unwrap_or(&s.path));
            self.preview_path(&s.title, &s.path, &path);
            if self.opts.verify_paths && self.exists.get(&path) == Some(&false) {
                self.warn(WarningSeverity::Warning, "series", Some(s.id), Some(&s.title), format!(
                    "Folder {path} does not exist for Kitsunarr; the series is imported but its files will show as missing until the path is fixed."
                ));
            }
            if s.media_type == SrcMediaType::Special {
                self.warn(
                    WarningSeverity::Info,
                    "series",
                    Some(s.id),
                    Some(&s.title),
                    "Tanukarr special/OVA entry imported as a series.",
                );
            }
            let kind = if s.series_type == SeriesType::Anime
                || matches!(
                    provider,
                    MetadataProvider::AniList | MetadataProvider::MyAnimeList
                ) {
                MediaKind::Anime
            } else {
                MediaKind::Series
            };
            let profile = self.profile_for("series", s.id, &s.title, s.quality_profile_id);
            let new = NewSeries {
                media_kind: kind,
                title: s.title.clone(),
                year: s.year,
                status: s.status,
                overview: s.overview.clone(),
                network: s.network.clone(),
                air_time: s.air_time.clone(),
                runtime: s.runtime,
                certification: s.certification.clone(),
                original_language: s.original_language,
                genres: s.genres.clone(),
                images: s.images.clone(),
                ratings: s.ratings.clone(),
                first_aired: s.first_aired.clone(),
                last_aired: s.last_aired.clone(),
                series_type: s.series_type,
                metadata_provider: provider,
                metadata_id: metadata_id.clone(),
                episode_ordering: s.ordering_id.clone(),
                monitored: s.monitored,
                monitor_new_items: s.monitor_new_items,
                season_folder: s.season_folder,
                use_scene_numbering: s.use_scene_numbering,
                quality_profile_id: profile,
                root_folder_path: root_folders::normalize_path(&root),
                path: path.clone(),
                added_at: s.added.clone().unwrap_or_else(kitsunarr_db::now),
                last_refreshed_at: s.last_info_sync.clone(),
                tags: self.map_tags(&s.tags),
                external_ids: Self::external_ids(s, (provider, &metadata_id)),
                alternate_titles: s
                    .alternate_titles
                    .iter()
                    .map(|(t, season)| (t.clone(), *season, Some("source".into())))
                    .collect(),
            };
            let existing = match self.mapped(conn, "series", s.id, "series").await? {
                Some(id) => Some(id),
                None => series::find_by_metadata(conn, provider, &metadata_id).await?,
            };
            if existing.is_none()
                && let Some(other) = series::find_by_path(conn, &path).await?
            {
                self.warn(WarningSeverity::Error, "series", Some(s.id), Some(&s.title), format!(
                    "Folder {path} already belongs to another Kitsunarr series (#{other}); this series was skipped."
                ));
                self.count("series", Act::Unsupported);
                continue;
            }
            let (target, write) = match existing {
                Some(id) if self.update_existing() => {
                    series::update_full(conn, id, &new).await?;
                    self.count("series", Act::Update);
                    (id, true)
                }
                Some(id) => {
                    self.count("series", Act::Skip);
                    (id, false)
                }
                None => {
                    let id = series::insert(conn, &new).await?;
                    self.count("series", Act::Create);
                    (id, true)
                }
            };
            self.series_map.insert(s.id, target);
            self.remember(conn, "series", s.id, target).await?;
            for (k, v) in &s.unmapped {
                self.keep_unmapped("series", Some(s.id), k, v.clone());
            }

            let episodes = eps_by_series.remove(&s.id).unwrap_or_default();
            if !write {
                let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
                    "SELECT id, season_number, episode_number FROM episodes WHERE series_id = ?",
                )
                .bind(target)
                .fetch_all(&mut *conn)
                .await?;
                let by_number: HashMap<(i64, i64), i64> =
                    rows.into_iter().map(|(id, s, e)| ((s, e), id)).collect();
                for e in &episodes {
                    if let Some(id) =
                        by_number.get(&(i64::from(e.season_number), i64::from(e.episode_number)))
                    {
                        self.episode_map.insert(e.id, *id);
                    }
                }
                continue;
            }
            for season in &s.seasons {
                series::upsert_season(
                    conn,
                    target,
                    &NewSeason {
                        season_number: season.number,
                        monitored: season.monitored,
                        title: None,
                        images: season.images.clone(),
                    },
                )
                .await?;
                self.count("seasons", Act::Create);
            }
            // Seasons referenced by episodes but missing from the season list.
            let known: HashSet<i32> = s.seasons.iter().map(|x| x.number).collect();
            let mut extra: Vec<i32> = episodes
                .iter()
                .map(|e| e.season_number)
                .filter(|n| !known.contains(n))
                .collect();
            extra.sort_unstable();
            extra.dedup();
            for n in extra {
                series::upsert_season(
                    conn,
                    target,
                    &NewSeason {
                        season_number: n,
                        monitored: n > 0,
                        title: None,
                        images: vec![],
                    },
                )
                .await?;
                self.count("seasons", Act::Create);
            }

            let mut file_map: HashMap<i64, i64> = HashMap::new();
            let mut seen_paths = HashSet::new();
            let files = files_by_series.remove(&s.id).unwrap_or_default();
            for f in &files {
                let quality_id = if Quality::by_id(f.quality_id).is_some() {
                    f.quality_id
                } else {
                    self.keep_unmapped("episodeFile", Some(f.id), "quality", json!(f.quality_id));
                    0
                };
                let id = series::upsert_episode_file(
                    conn,
                    &NewEpisodeFile {
                        series_id: target,
                        season_number: f.season_number,
                        relative_path: f.relative_path.clone(),
                        size: f.size,
                        added_at: f.date_added.clone().unwrap_or_else(kitsunarr_db::now),
                        scene_name: f.scene_name.clone(),
                        release_group: f.release_group.clone(),
                        quality_id,
                        revision_version: f.revision.version,
                        revision_real: f.revision.real,
                        is_repack: f.revision.is_repack,
                        languages: f.languages.clone(),
                        media_info: f.media_info.clone(),
                        original_path: f.original_file_path.clone(),
                        indexer_flags: f.indexer_flags,
                        release_type: f.release_type,
                    },
                )
                .await?;
                file_map.insert(f.id, id);
                if seen_paths.insert(f.relative_path.clone()) {
                    self.count("episodeFiles", Act::Create);
                }
                for (k, v) in &f.unmapped {
                    self.keep_unmapped("episodeFile", Some(f.id), k, v.clone());
                }
            }

            // Duplicate (season, episode) rows: keep the one with a file, then the lowest id.
            let mut chosen: BTreeMap<(i32, i32), &SrcEpisode> = BTreeMap::new();
            for e in &episodes {
                let key = (e.season_number, e.episode_number);
                match chosen.get(&key) {
                    Some(prev)
                        if prev.episode_file_id.is_some()
                            || (e.episode_file_id.is_none() && prev.id < e.id) =>
                    {
                        self.warn(
                            WarningSeverity::Info,
                            "episodes",
                            Some(e.id),
                            Some(&s.title),
                            format!("Duplicate episode S{:02}E{:02} merged.", key.0, key.1),
                        );
                    }
                    _ => {
                        chosen.insert(key, e);
                    }
                }
            }
            let (mut monitored, mut linked, mut orphan_links) = (0i64, 0i64, 0u64);
            for e in chosen.values() {
                let file_id = e.episode_file_id.and_then(|f| file_map.get(&f).copied());
                if e.episode_file_id.is_some() && file_id.is_none() {
                    orphan_links += 1;
                }
                series::upsert_episode(
                    conn,
                    &NewEpisode {
                        series_id: target,
                        season_number: e.season_number,
                        episode_number: e.episode_number,
                        absolute_episode_number: e.absolute_episode_number,
                        scene_season_number: e.scene_season_number,
                        scene_episode_number: e.scene_episode_number,
                        scene_absolute_episode_number: e.scene_absolute_episode_number,
                        unverified_scene_numbering: e.unverified_scene_numbering,
                        title: e.title.clone(),
                        overview: e.overview.clone(),
                        air_date: e.air_date.clone(),
                        air_date_utc: e.air_date_utc.clone(),
                        runtime: e.runtime,
                        monitored: e.monitored,
                        episode_file_id: file_id,
                        provider_episode_id: e.provider_episode_id.clone(),
                        last_search_at: e.last_search_time.clone(),
                    },
                )
                .await?;
                self.count("episodes", Act::Create);
                monitored += i64::from(e.monitored);
                linked += i64::from(file_id.is_some());
                // Tanukarr's watched state (migration 248), kept as "migrated".
                if e.unmapped.contains_key("watched") {
                    let at = e
                        .unmapped
                        .get("watchedatutc")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    sqlx::query(
                        "UPDATE episodes SET watched = 1, watched_at = ?, watched_source = 'migrated' \
                         WHERE series_id = ? AND season_number = ? AND episode_number = ?",
                    )
                    .bind(at)
                    .bind(target)
                    .bind(e.season_number)
                    .bind(e.episode_number)
                    .execute(&mut *conn)
                    .await?;
                }
            }
            for e in &episodes {
                if let Some(target_ep) = chosen.get(&(e.season_number, e.episode_number)) {
                    // Every duplicate maps onto the kept row so history stays attached.
                    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM episodes WHERE series_id = ? AND season_number = ? AND episode_number = ?")
                        .bind(target)
                        .bind(target_ep.season_number)
                        .bind(target_ep.episode_number)
                        .fetch_optional(&mut *conn)
                        .await?;
                    if let Some(id) = id {
                        self.episode_map.insert(e.id, id);
                    }
                }
            }
            if orphan_links > 0 {
                self.warn(WarningSeverity::Info, "episodes", Some(s.id), Some(&s.title), format!(
                    "{orphan_links} episode(s) referenced file records that no longer exist in the source; they were imported without a file."
                ));
            }
            self.expects.push(Expect {
                target,
                episodes: chosen.len() as i64,
                files: seen_paths.len() as i64,
                monitored,
                linked,
                path,
            });
        }
        (self.progress)("series", total, total);
        Ok(())
    }

    async fn movies(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        if !self.enabled("movies") {
            return Ok(());
        }
        let movies: Vec<&SrcSeries> = self
            .snap
            .series
            .iter()
            .filter(|s| s.media_type == SrcMediaType::Movie)
            .collect();
        for s in movies {
            let (provider, metadata_id) = s.identity();
            let path = self.mapper.map(&s.path);
            let root = self
                .mapper
                .map(s.root_folder_path.as_deref().unwrap_or(&s.path));
            self.preview_path(&s.title, &s.path, &path);
            let profile = self.profile_for("movies", s.id, &s.title, s.quality_profile_id);
            let status = match s.status {
                SeriesStatus::Ended => MovieStatus::Released,
                SeriesStatus::Upcoming => MovieStatus::Announced,
                SeriesStatus::Continuing => MovieStatus::InCinemas,
                SeriesStatus::Deleted => MovieStatus::Deleted,
                SeriesStatus::Unknown => MovieStatus::Unknown,
            };
            let new = NewMovie {
                title: s.title.clone(),
                original_title: s
                    .original_title
                    .clone()
                    .or_else(|| s.alternate_titles.first().map(|a| a.0.clone())),
                year: s.year,
                status,
                overview: s.overview.clone(),
                studio: s.network.clone(),
                runtime: s.runtime,
                certification: s.certification.clone(),
                original_language: s.original_language,
                genres: s.genres.clone(),
                images: s.images.clone(),
                ratings: s.ratings.clone(),
                in_cinemas: s.first_aired.clone(),
                digital_release: None,
                physical_release: None,
                metadata_provider: provider,
                metadata_id: metadata_id.clone(),
                monitored: s.monitored,
                minimum_availability: MinimumAvailability::Released,
                quality_profile_id: profile,
                root_folder_path: root_folders::normalize_path(&root),
                path: path.clone(),
                added_at: s.added.clone().unwrap_or_else(kitsunarr_db::now),
                last_refreshed_at: s.last_info_sync.clone(),
                tags: self.map_tags(&s.tags),
                external_ids: Self::external_ids(s, (provider, &metadata_id)),
            };
            let existing = match self.mapped(conn, "movie", s.id, "movies").await? {
                Some(id) => Some(id),
                None => movies::find_by_metadata(conn, provider, &metadata_id).await?,
            };
            let (target, write) = match existing {
                Some(id) if self.update_existing() => {
                    movies::update_full(conn, id, &new).await?;
                    self.count("movies", Act::Update);
                    (id, true)
                }
                Some(id) => {
                    self.count("movies", Act::Skip);
                    (id, false)
                }
                None => {
                    if movies::find_by_path(conn, &path).await?.is_some() {
                        self.warn(
                            WarningSeverity::Error,
                            "movies",
                            Some(s.id),
                            Some(&s.title),
                            format!("Folder {path} already belongs to another movie; skipped."),
                        );
                        self.count("movies", Act::Unsupported);
                        continue;
                    }
                    let id = movies::insert(conn, &new).await?;
                    self.count("movies", Act::Create);
                    (id, true)
                }
            };
            self.movie_by_series.insert(s.id, target);
            self.remember(conn, "movie", s.id, target).await?;
            if write {
                self.movies_written.push(target);
                let files: Vec<&SrcEpisodeFile> = self
                    .snap
                    .episode_files
                    .iter()
                    .filter(|f| f.series_id == s.id)
                    .collect();
                if files.len() > 1 {
                    self.warn(WarningSeverity::Warning, "movieFiles", Some(s.id), Some(&s.title), format!(
                        "{} files were linked to this movie in Tanukarr; only the newest is kept as the movie file.", files.len()
                    ));
                }
                if let Some(f) = files.iter().max_by(|a, b| a.date_added.cmp(&b.date_added)) {
                    movies::upsert_file(
                        conn,
                        &NewMovieFile {
                            movie_id: target,
                            relative_path: f.relative_path.clone(),
                            size: f.size,
                            added_at: f.date_added.clone().unwrap_or_else(kitsunarr_db::now),
                            scene_name: f.scene_name.clone(),
                            release_group: f.release_group.clone(),
                            quality_id: if Quality::by_id(f.quality_id).is_some() {
                                f.quality_id
                            } else {
                                0
                            },
                            revision_version: f.revision.version,
                            revision_real: f.revision.real,
                            is_repack: f.revision.is_repack,
                            languages: f.languages.clone(),
                            media_info: f.media_info.clone(),
                            original_path: f.original_file_path.clone(),
                            edition: None,
                        },
                    )
                    .await?;
                    self.count("movieFiles", Act::Create);
                }
            }
            for (k, v) in &s.unmapped {
                self.keep_unmapped("movie", Some(s.id), k, v.clone());
            }
        }
        Ok(())
    }

    async fn collections(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        if !(self.enabled("series") || self.enabled("movies")) {
            return Ok(());
        }
        // key → (title, kind, members)
        let mut groups: BTreeMap<String, GroupPlan> = BTreeMap::new();
        for s in &self.snap.series {
            let member = match self.movie_by_series.get(&s.id) {
                Some(id) => Member::Movie(*id),
                None => match self.series_map.get(&s.id) {
                    Some(id) => Member::Series(*id),
                    None => continue,
                },
            };
            if let Some(g) = s.group.as_ref().filter(|g| g.grouping_mode != 2)
                && let Some(gid) = &g.group_id
            {
                let title = g.group_title.clone().unwrap_or_else(|| gid.clone());
                let label = (g.member_kind == 1)
                    .then(|| format!("Film {:02}", g.movie_number.unwrap_or(0)));
                groups
                    .entry(format!("group:{gid}"))
                    .or_insert_with(|| (title, "animeGroup", Vec::new()))
                    .2
                    .push((member, g.position(), g.role(), label));
            }
            if let Some(ct) = &s.collection_title {
                groups
                    .entry(format!("collection:{}", ct.to_lowercase()))
                    .or_insert_with(|| (ct.clone(), "manual", Vec::new()))
                    .2
                    .push((member, i64::from(s.year.unwrap_or(0)), "member", None));
            }
        }
        for (_, (title, kind, mut members)) in groups {
            members.sort_by_key(|m| m.1);
            let id = match collections::find_by_title(conn, &title, kind).await? {
                Some(id) => {
                    self.count("collections", Act::Skip);
                    id
                }
                None => {
                    self.count("collections", Act::Create);
                    collections::insert(
                        conn,
                        &NewCollection {
                            title: title.clone(),
                            kind: kind.into(),
                            metadata_provider: None,
                            metadata_id: None,
                            overview: None,
                            images: vec![],
                        },
                    )
                    .await?
                }
            };
            for (i, (member, _, role, label)) in members.iter().enumerate() {
                collections::add_member(
                    conn,
                    id,
                    *member,
                    i as i32 + 1,
                    Some(role),
                    label.as_deref(),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn history(&mut self, conn: &mut SqliteConnection) -> Result<(), MigrateError> {
        if !self.enabled("history") || !self.opts.include_history {
            return Ok(());
        }
        let total = self.snap.history.len() as u64;
        for (i, h) in self.snap.history.iter().enumerate() {
            if i % 1000 == 0 {
                (self.progress)("history", i as u64, total);
            }
            if migration_runs::lookup(conn, self.fp, "history", &h.id.to_string())
                .await?
                .is_some()
            {
                self.count("history", Act::Skip);
                continue;
            }
            let movie_id = h
                .series_id
                .and_then(|s| self.movie_by_series.get(&s).copied());
            let series_id = if movie_id.is_some() {
                None
            } else {
                h.series_id.and_then(|s| self.series_map.get(&s).copied())
            };
            if movie_id.is_none() && series_id.is_none() {
                self.count("history", Act::Skip);
                continue;
            }
            let episode_id = if series_id.is_some() {
                h.episode_id.and_then(|e| self.episode_map.get(&e).copied())
            } else {
                None
            };
            let id = history::insert(
                conn,
                &history::NewHistory {
                    event_type: h.event_type.clone(),
                    date: h.date.clone(),
                    source_title: h.source_title.clone(),
                    series_id,
                    episode_id,
                    movie_id,
                    quality_id: h.quality_id,
                    revision: h.revision.map(
                        |r| json!({"version": r.version, "real": r.real, "isRepack": r.is_repack}),
                    ),
                    languages: h.languages.clone(),
                    custom_format_score: None,
                    download_id: h.download_id.clone(),
                    data: h.data.clone(),
                },
            )
            .await?;
            self.count("history", Act::Create);
            self.remember(conn, "history", h.id, id).await?;
        }
        Ok(())
    }

    fn blocking(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.opts.verify_paths
            && !self.expects.is_empty()
            && self
                .expects
                .iter()
                .all(|e| self.exists.get(&e.path) == Some(&false))
        {
            out.push(
                "None of the mapped series folders exist for Kitsunarr. Add path mappings (source path → Kitsunarr path) \
                 or fix the container volumes, or disable path verification to import anyway."
                    .into(),
            );
        }
        out
    }

    async fn validate(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<Vec<ValidationCheck>, MigrateError> {
        let mut checks = Vec::new();
        let fk: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&mut *conn)
            .await?;
        checks.push(ValidationCheck {
            name: "Referential integrity".into(),
            passed: fk == 0,
            expected: "0 violations".into(),
            actual: format!("{fk} violations"),
        });

        let rows: Vec<(i64, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT s.id, \
               (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id), \
               (SELECT COUNT(*) FROM episode_files f WHERE f.series_id = s.id), \
               (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id AND e.monitored = 1), \
               (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id AND e.episode_file_id IS NOT NULL) \
             FROM series s",
        )
        .fetch_all(&mut *conn)
        .await?;
        let actual: HashMap<i64, (i64, i64, i64, i64)> = rows
            .into_iter()
            .map(|(id, a, b, c, d)| (id, (a, b, c, d)))
            .collect();
        let paths: HashMap<i64, String> =
            sqlx::query_as::<_, (i64, String)>("SELECT id, path FROM series")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .collect();
        let sum = |f: &dyn Fn(&Expect) -> i64| self.expects.iter().map(f).sum::<i64>();
        let got = |f: &dyn Fn(&SeriesCounts) -> i64| {
            self.expects
                .iter()
                .map(|e| actual.get(&e.target).map_or(0, f))
                .sum::<i64>()
        };
        let present = self
            .expects
            .iter()
            .filter(|e| actual.contains_key(&e.target))
            .count();
        checks.push(ValidationCheck {
            name: "Series records".into(),
            passed: present == self.expects.len(),
            expected: self.expects.len().to_string(),
            actual: present.to_string(),
        });
        for (name, exp, act) in [
            ("Episodes", sum(&|e| e.episodes), got(&|a| a.0)),
            ("Episode files", sum(&|e| e.files), got(&|a| a.1)),
            ("Monitored episodes", sum(&|e| e.monitored), got(&|a| a.2)),
            ("Episode ↔ file links", sum(&|e| e.linked), got(&|a| a.3)),
        ] {
            // Updating an existing series may legitimately leave extra rows; creating must match exactly.
            checks.push(ValidationCheck {
                name: name.into(),
                passed: act >= exp && (act == exp || self.update_existing()),
                expected: exp.to_string(),
                actual: act.to_string(),
            });
        }
        let wrong_paths = self
            .expects
            .iter()
            .filter(|e| paths.get(&e.target).is_some_and(|p| *p != e.path))
            .count();
        checks.push(ValidationCheck {
            name: "Series paths preserved".into(),
            passed: wrong_paths == 0,
            expected: "0 differences".into(),
            actual: format!("{wrong_paths} differences"),
        });
        let mut movies_present = 0;
        for id in &self.movies_written {
            let found: Option<i64> = sqlx::query_scalar("SELECT id FROM movies WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?;
            movies_present += usize::from(found.is_some());
        }
        checks.push(ValidationCheck {
            name: "Movie records".into(),
            passed: movies_present == self.movies_written.len(),
            expected: self.movies_written.len().to_string(),
            actual: movies_present.to_string(),
        });
        Ok(checks)
    }

    fn summary(&self) -> Vec<EntityPlanSummary> {
        ORDER
            .iter()
            .filter_map(|k| self.counts.get(k).cloned())
            .collect()
    }
}

fn provider_table(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Indexer => "indexers",
        ProviderKind::DownloadClient => "download_clients",
        ProviderKind::Notification => "notifications",
        ProviderKind::ImportList => "import_lists",
    }
}

/// Read-only existence checks for every mapped path the migration references.
fn check_paths(paths: Vec<String>) -> HashMap<String, bool> {
    paths
        .into_iter()
        .map(|p| {
            let exists = Path::new(&p).is_dir();
            (p, exists)
        })
        .collect()
}

fn validate_options(options: &MigrationOptions) -> Result<(), MigrateError> {
    for m in &options.path_mappings {
        let target = m.target_prefix.trim();
        if target.is_empty() || m.source_prefix.trim().is_empty() {
            return Err(MigrateError::InvalidOptions(
                "path mappings need both a source and a Kitsunarr path".into(),
            ));
        }
        if !Path::new(target).is_absolute() || target.split(['/', '\\']).any(|seg| seg == "..") {
            return Err(MigrateError::InvalidOptions(format!(
                "`{target}` must be an absolute path without `..`"
            )));
        }
    }
    for e in &options.entities {
        if !kitsunarr_domain::api::migration::MIGRATION_ENTITY_GROUPS.contains(&e.as_str()) {
            return Err(MigrateError::InvalidOptions(format!(
                "unknown entity group `{e}`"
            )));
        }
    }
    Ok(())
}

/// Runs a dry run or applies the migration.
pub async fn run(
    db: &Database,
    source: &LoadedSource,
    options: &MigrationOptions,
    mode: Mode,
    progress: Progress<'_>,
) -> Result<Outcome, MigrateError> {
    validate_options(options)?;
    let mapper = PathMapper::new(&options.path_mappings);
    let snap = &source.snapshot;
    let exists = if options.verify_paths {
        let mut wanted: Vec<String> = snap.series.iter().map(|s| mapper.map(&s.path)).collect();
        wanted.extend(
            snap.root_paths()
                .iter()
                .map(|p| root_folders::normalize_path(&mapper.map(p))),
        );
        tokio::task::spawn_blocking(move || check_paths(wanted))
            .await
            .map_err(|e| MigrateError::Internal(e.to_string()))?
    } else {
        HashMap::new()
    };

    let backup_path = match &mode {
        Mode::Apply { backup_dir } => {
            progress("backup", 0, 1);
            let dest = maintenance::unique_backup_path(backup_dir, "pre-migration");
            Some(
                maintenance::backup_to(db, &dest)
                    .await?
                    .display()
                    .to_string(),
            )
        }
        Mode::DryRun => None,
    };

    let detected = &source.detected;
    let options_json = serde_json::to_value(options).unwrap_or(Value::Null);
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let run_id = migration_runs::start(
        &mut *tx,
        migration_runs::NewRun {
            source_app: detected.app.as_str(),
            source_method: detected.method.as_str(),
            source_version: &detected
                .product_version
                .clone()
                .unwrap_or_else(|| detected.app_version.clone()),
            source_instance: detected.instance_name.as_deref(),
            source_fingerprint: &detected.fingerprint,
            options: &options_json,
            backup_path: backup_path.as_deref(),
        },
    )
    .await?;

    let mut imp = Importer {
        snap,
        fp: &detected.fingerprint,
        run_id,
        opts: options,
        mapper,
        exists,
        groups: options.entities.iter().cloned().collect(),
        counts: BTreeMap::new(),
        warnings: Vec::new(),
        dropped_warnings: 0,
        paths: Vec::new(),
        tag_map: HashMap::new(),
        cf_map: HashMap::new(),
        profile_map: HashMap::new(),
        client_map: HashMap::new(),
        indexer_map: HashMap::new(),
        series_map: HashMap::new(),
        episode_map: HashMap::new(),
        movie_by_series: HashMap::new(),
        default_profile: 0,
        unmapped: Vec::new(),
        expects: Vec::new(),
        movies_written: Vec::new(),
        progress,
    };
    for note in &source.notes {
        imp.warn(WarningSeverity::Info, "source", None, None, note.clone());
    }
    if snap.internal_settings_skipped > 0 {
        imp.warn(WarningSeverity::Info, "settings", None, None, format!(
            "{} internal source settings (encryption salts, client identifiers) were intentionally not imported.",
            snap.internal_settings_skipped
        ));
    }

    let result: Result<(), MigrateError> = async {
        let conn: &mut SqliteConnection = &mut tx;
        progress("configuration", 0, 1);
        imp.tags(conn).await?;
        imp.root_folders(conn).await?;
        imp.quality_definitions(conn).await?;
        imp.custom_formats(conn).await?;
        imp.quality_profiles(conn).await?;
        imp.providers(conn, ProviderKind::DownloadClient, "downloadClients")
            .await?;
        imp.providers(conn, ProviderKind::Indexer, "indexers")
            .await?;
        imp.providers(conn, ProviderKind::Notification, "notifications")
            .await?;
        imp.providers(conn, ProviderKind::ImportList, "importLists")
            .await?;
        imp.delay_and_release_profiles(conn).await?;
        imp.settings(conn).await?;
        imp.series(conn).await?;
        imp.movies(conn).await?;
        imp.collections(conn).await?;
        imp.series_naming_options(conn).await?;
        imp.history(conn).await?;
        Ok(())
    }
    .await;

    if let Err(e) = result {
        let _ = tx.rollback().await;
        if matches!(mode, Mode::Apply { .. }) {
            record_failure(
                db,
                source,
                &options_json,
                backup_path.as_deref(),
                &e.to_string(),
            )
            .await;
        }
        return Err(e);
    }

    progress("validation", 0, 1);
    let validation = imp.validate(&mut tx).await?;
    for (entity, source_id, field, value) in &imp.unmapped {
        migration_runs::record_unmapped(
            &mut tx,
            run_id,
            entity,
            source_id.as_deref(),
            field,
            value,
        )
        .await?;
    }
    if imp.dropped_warnings > 0 {
        let n = imp.dropped_warnings;
        imp.dropped_warnings = 0;
        imp.warn(
            WarningSeverity::Info,
            "report",
            None,
            None,
            format!("{n} further warnings were omitted."),
        );
    }
    let blocking = imp.blocking();
    let summary = imp.summary();
    let mut warnings = imp.warnings.clone();
    warnings.sort_by_key(|w| std::cmp::Reverse(w.severity));
    let paths = imp.paths.clone();
    let passed = validation.iter().all(|c| c.passed);

    match mode {
        Mode::DryRun => {
            tx.rollback().await?;
            Ok(Outcome {
                summary,
                warnings,
                paths,
                blocking,
                validation,
                run_id: None,
                backup_path: None,
            })
        }
        Mode::Apply { .. } if !passed || !blocking.is_empty() => {
            tx.rollback().await?;
            let reason = if passed {
                blocking.join(" ")
            } else {
                "post-import validation failed".to_string()
            };
            record_failure(db, source, &options_json, backup_path.as_deref(), &reason).await;
            Err(MigrateError::ValidationFailed {
                reason,
                checks: validation,
            })
        }
        Mode::Apply { .. } => {
            tx.commit().await?;
            let report = json!({ "warnings": warnings, "validation": validation });
            migration_runs::finish(
                db.pool(),
                run_id,
                "completed",
                &json!(summary),
                &report,
                None,
            )
            .await?;
            Ok(Outcome {
                summary,
                warnings,
                paths,
                blocking,
                validation,
                run_id: Some(run_id),
                backup_path,
            })
        }
    }
}

async fn record_failure(
    db: &Database,
    source: &LoadedSource,
    options: &Value,
    backup: Option<&str>,
    error: &str,
) {
    let d = &source.detected;
    let version = d
        .product_version
        .clone()
        .unwrap_or_else(|| d.app_version.clone());
    let run = migration_runs::start(
        db.pool(),
        migration_runs::NewRun {
            source_app: d.app.as_str(),
            source_method: d.method.as_str(),
            source_version: &version,
            source_instance: d.instance_name.as_deref(),
            source_fingerprint: &d.fingerprint,
            options,
            backup_path: backup,
        },
    )
    .await;
    if let Ok(id) = run {
        let _ =
            migration_runs::finish(db.pool(), id, "failed", &json!([]), &json!({}), Some(error))
                .await;
    }
}
