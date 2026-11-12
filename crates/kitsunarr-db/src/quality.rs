//! Quality definitions, quality profiles and custom formats.

use kitsunarr_domain::api::settings::{
    CustomFormat, CustomFormatSpecification, CustomQualityCreate, FormatScore, OrderedF64,
    QualityDefinition, QualityGroup, QualityProfile, QualityProfileItem,
};
use kitsunarr_domain::quality::{CUSTOM_QUALITY_BASE, QUALITIES, Quality, QualitySource};
use sqlx::{FromRow, SqliteExecutor};

use crate::{DbError, DbResult, from_json, to_json};

// ---------------------------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------------------------

#[derive(FromRow)]
struct DefinitionRow {
    quality_id: i64,
    title: String,
    min_size: Option<f64>,
    preferred_size: Option<f64>,
    max_size: Option<f64>,
    source: Option<String>,
    resolution: Option<i64>,
    codec: String,
    title_pattern: String,
    is_retired: bool,
}

fn source_of(raw: Option<&str>) -> Option<QualitySource> {
    raw.and_then(|s| serde_json::from_value(serde_json::Value::String(s.into())).ok())
}

fn source_name(s: QualitySource) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Registers the stored custom presets (`Quality.Register` at start-up).
pub async fn register_custom(ex: impl SqliteExecutor<'_>) -> DbResult<()> {
    let rows: Vec<(i64, String, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT quality_id, title, source, resolution FROM quality_definitions WHERE quality_id >= ?",
    )
    .bind(CUSTOM_QUALITY_BASE)
    .fetch_all(ex)
    .await?;
    for (id, title, source, resolution) in rows {
        Quality::register(
            id as i32,
            &title,
            source_of(source.as_deref()).unwrap_or(QualitySource::Unknown),
            resolution.unwrap_or(0) as i32,
        );
    }
    Ok(())
}

/// Tanukarr's resolutions for presets.
const RESOLUTIONS: [i32; 6] = [0, 480, 576, 720, 1080, 2160];

/// Validates a preset's codec and title pattern.
fn check_criteria(codec: &str, pattern: &str) -> DbResult<()> {
    if !codec.is_empty() && !kitsunarr_domain::codec::FAMILIES.contains(&codec) {
        return Err(DbError::Invalid(format!("unknown codec family {codec}")));
    }
    if pattern.chars().count() > 256 {
        return Err(DbError::Invalid(
            "the title pattern is limited to 256 characters".into(),
        ));
    }
    if !pattern.is_empty() && regex::Regex::new(pattern).is_err() {
        return Err(DbError::Invalid(
            "the title pattern is not a valid regular expression".into(),
        ));
    }
    Ok(())
}

pub async fn list_definitions(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<QualityDefinition>> {
    let rows: Vec<DefinitionRow> = sqlx::query_as("SELECT * FROM quality_definitions")
        .fetch_all(ex)
        .await?;
    let mut defs: Vec<QualityDefinition> = rows
        .into_iter()
        .map(|r| {
            let id = r.quality_id as i32;
            QualityDefinition {
                quality_id: id,
                title: r.title,
                weight: Quality::by_id(id).map_or(0, |q| q.weight),
                min_size: r.min_size.map(OrderedF64),
                preferred_size: r.preferred_size.map(OrderedF64),
                max_size: r.max_size.map(OrderedF64),
                source: source_of(r.source.as_deref()),
                resolution: r.resolution.map(|v| v as i32),
                codec: r.codec,
                title_pattern: r.title_pattern,
                is_retired: r.is_retired,
            }
        })
        .collect();
    defs.sort_by_key(|d| (d.weight, d.quality_id));
    Ok(defs)
}

pub async fn upsert_definition(ex: impl SqliteExecutor<'_>, d: &QualityDefinition) -> DbResult<()> {
    if d.quality_id < CUSTOM_QUALITY_BASE && Quality::by_id(d.quality_id).is_none() {
        return Err(DbError::Invalid(format!(
            "unknown quality id {}",
            d.quality_id
        )));
    }
    let (min, pref, max) = (
        d.min_size.map(|v| v.0),
        d.preferred_size.map(|v| v.0),
        d.max_size.map(|v| v.0),
    );
    if let (Some(a), Some(b)) = (min, max)
        && a > b
    {
        return Err(DbError::Invalid(
            "minimum size must not exceed maximum size".into(),
        ));
    }
    check_criteria(&d.codec, &d.title_pattern)?;
    let custom = d.quality_id >= CUSTOM_QUALITY_BASE;
    // A standard quality keeps its own source and resolution.
    let (source, resolution) = if custom {
        let source = d
            .source
            .ok_or(DbError::Invalid("the source is required".into()))?;
        let resolution =
            d.resolution
                .filter(|r| RESOLUTIONS.contains(r))
                .ok_or(DbError::Invalid(
                    "the resolution must be 0, 480, 576, 720, 1080 or 2160".into(),
                ))?;
        (Some(source_name(source)), Some(resolution))
    } else {
        (None, None)
    };
    sqlx::query(
        "INSERT INTO quality_definitions (quality_id, title, min_size, preferred_size, max_size, source, resolution, codec, title_pattern) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(quality_id) DO UPDATE SET title = excluded.title, min_size = excluded.min_size, \
         preferred_size = excluded.preferred_size, max_size = excluded.max_size, source = excluded.source, \
         resolution = excluded.resolution, codec = excluded.codec, title_pattern = excluded.title_pattern",
    )
    .bind(d.quality_id)
    .bind(d.title.trim())
    .bind(min)
    .bind(pref)
    .bind(max)
    .bind(&source)
    .bind(resolution)
    .bind(d.codec.trim())
    .bind(d.title_pattern.trim())
    .execute(ex)
    .await?;
    if custom && let (Some(s), Some(r)) = (d.source, resolution) {
        Quality::register(d.quality_id, d.title.trim(), s, r);
    }
    Ok(())
}

/// `QualityDefinitionService.Add`: a custom preset (id 10000 and up), appended to every profile
/// as not allowed; `copy_from_id` copies a quality's size limits, globally and per profile.
pub async fn create_custom(
    db: &crate::Database,
    c: &CustomQualityCreate,
) -> DbResult<QualityDefinition> {
    let title = c.title.trim();
    if title.is_empty() || title.chars().count() > 100 {
        return Err(DbError::Invalid(
            "the title must have 1 to 100 characters".into(),
        ));
    }
    let defs = list_definitions(db.pool()).await?;
    if defs.iter().any(|d| d.title.eq_ignore_ascii_case(title))
        || QUALITIES.iter().any(|q| q.name.eq_ignore_ascii_case(title))
    {
        return Err(DbError::Conflict(format!("a quality named {title} exists")));
    }
    let source = c
        .source
        .ok_or(DbError::Invalid("the source is required".into()))?;
    let resolution = c
        .resolution
        .filter(|r| RESOLUTIONS.contains(r))
        .ok_or(DbError::Invalid(
            "the resolution must be 0, 480, 576, 720, 1080 or 2160".into(),
        ))?;
    check_criteria(c.codec.trim(), c.title_pattern.trim())?;
    let copy = match c.copy_from_id {
        Some(id) => Some(
            defs.iter()
                .find(|d| d.quality_id == id)
                .cloned()
                .ok_or(DbError::Invalid(format!("unknown quality id {id}")))?,
        ),
        None => None,
    };
    let mut tx = db.pool().begin().await?;
    let max: Option<i64> = sqlx::query_scalar("SELECT MAX(quality_id) FROM quality_definitions")
        .fetch_one(&mut *tx)
        .await?;
    let id = (max.unwrap_or(0) as i32 + 1).max(CUSTOM_QUALITY_BASE);
    let base = QUALITIES
        .iter()
        .find(|q| q.source == source && q.resolution == resolution);
    let def = QualityDefinition {
        quality_id: id,
        title: title.to_string(),
        weight: 19,
        min_size: copy
            .as_ref()
            .map_or(base.map(|b| OrderedF64(b.default_min_size)), |d| d.min_size),
        preferred_size: copy
            .as_ref()
            .map_or(base.map(|b| OrderedF64(b.default_preferred_size)), |d| {
                d.preferred_size
            }),
        max_size: copy
            .as_ref()
            .map_or(base.and_then(|b| b.default_max_size.map(OrderedF64)), |d| {
                d.max_size
            }),
        source: Some(source),
        resolution: Some(resolution),
        codec: c.codec.trim().to_string(),
        title_pattern: c.title_pattern.trim().to_string(),
        is_retired: false,
    };
    upsert_definition(&mut *tx, &def).await?;
    for mut p in list_profiles(&mut *tx).await? {
        p.items.push(q(id, false));
        if let Some(from) = c.copy_from_id
            && let Some(l) = p.size_limits.iter().find(|l| l.quality_id == from).cloned()
        {
            p.size_limits
                .push(kitsunarr_domain::api::settings::QualitySizeLimit {
                    quality_id: id,
                    ..l
                });
        }
        sqlx::query("UPDATE quality_profiles SET items = ?, size_limits = ? WHERE id = ?")
            .bind(to_json(&p.items)?)
            .bind(to_json(&p.size_limits)?)
            .bind(p.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(def)
}

/// `QualityDefinitionService.Delete` (retire): refused for Unknown and while a profile allows
/// the quality or uses it as its cutoff.
pub async fn retire(db: &crate::Database, id: i32) -> DbResult<()> {
    if id == 0 {
        return Err(DbError::Invalid(
            "the Unknown quality cannot be retired".into(),
        ));
    }
    let using: Vec<String> = list_profiles(db.pool())
        .await?
        .into_iter()
        .filter(|p| {
            p.cutoff == id
                || p.items.iter().any(|i| {
                    i.allowed
                        && (i.quality_id == Some(id)
                            || i.group
                                .as_ref()
                                .is_some_and(|g| g.quality_ids.contains(&id)))
                })
        })
        .map(|p| p.name)
        .collect();
    if !using.is_empty() {
        return Err(DbError::Conflict(format!(
            "the quality is allowed in: {}",
            using.join(", ")
        )));
    }
    set_retired(db, id, true).await
}

/// Tanukarr's profile guards for presets: retired qualities cannot be allowed, and custom
/// presets a profile omits are kept in it (not allowed).
pub async fn prepare_profile(db: &crate::Database, p: &mut QualityProfile) -> DbResult<()> {
    let defs = list_definitions(db.pool()).await?;
    let ids_of = |i: &QualityProfileItem| -> Vec<i32> {
        match (&i.quality_id, &i.group) {
            (Some(q), _) => vec![*q],
            (None, Some(g)) => g.quality_ids.clone(),
            _ => Vec::new(),
        }
    };
    for item in p.items.iter().filter(|i| i.allowed) {
        for id in ids_of(item) {
            if let Some(d) = defs.iter().find(|d| d.quality_id == id && d.is_retired) {
                return Err(DbError::Invalid(format!(
                    "{} is retired and cannot be allowed",
                    d.title
                )));
            }
        }
    }
    let present: std::collections::HashSet<i32> = p.items.iter().flat_map(ids_of).collect();
    for d in defs
        .iter()
        .filter(|d| d.quality_id >= CUSTOM_QUALITY_BASE && !present.contains(&d.quality_id))
    {
        p.items.push(q(d.quality_id, false));
    }
    Ok(())
}

pub async fn set_retired(db: &crate::Database, id: i32, retired: bool) -> DbResult<()> {
    let r = sqlx::query("UPDATE quality_definitions SET is_retired = ? WHERE quality_id = ?")
        .bind(retired)
        .bind(id)
        .execute(db.pool())
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------------------------

#[derive(FromRow)]
struct ProfileRow {
    id: i64,
    name: String,
    upgrade_allowed: bool,
    cutoff: i64,
    items: String,
    min_format_score: i64,
    cutoff_format_score: i64,
    min_upgrade_format_score: i64,
    format_items: String,
    size_limits: String,
    codec_multipliers: String,
}

impl ProfileRow {
    fn into_profile(self) -> DbResult<QualityProfile> {
        Ok(QualityProfile {
            id: self.id,
            name: self.name,
            upgrade_allowed: self.upgrade_allowed,
            cutoff: self.cutoff as i32,
            items: from_json(&self.items)?,
            min_format_score: self.min_format_score as i32,
            cutoff_format_score: self.cutoff_format_score as i32,
            min_upgrade_format_score: self.min_upgrade_format_score as i32,
            format_items: from_json(&self.format_items)?,
            size_limits: from_json(&self.size_limits)?,
            codec_multipliers: from_json(&self.codec_multipliers)?,
        })
    }
}

pub async fn list_profiles(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<QualityProfile>> {
    let rows: Vec<ProfileRow> = sqlx::query_as("SELECT * FROM quality_profiles ORDER BY id")
        .fetch_all(ex)
        .await?;
    rows.into_iter().map(ProfileRow::into_profile).collect()
}

pub async fn get_profile(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<QualityProfile> {
    let row: Option<ProfileRow> = sqlx::query_as("SELECT * FROM quality_profiles WHERE id = ?")
        .bind(id)
        .fetch_optional(ex)
        .await?;
    row.ok_or(DbError::NotFound)?.into_profile()
}

/// Structural validation shared by create/update and migration.
pub fn validate_profile(p: &QualityProfile) -> Result<(), DbError> {
    if p.name.trim().is_empty() {
        return Err(DbError::Invalid("profile name is required".into()));
    }
    let mut seen = std::collections::HashSet::new();
    let mut cutoff_found = false;
    let mut any_allowed = false;
    for item in &p.items {
        match (&item.quality_id, &item.group) {
            (Some(q), None) => {
                if Quality::by_id(*q).is_none() {
                    return Err(DbError::Invalid(format!("unknown quality id {q}")));
                }
                if !seen.insert(*q) {
                    return Err(DbError::Invalid(format!("quality {q} appears twice")));
                }
                cutoff_found |= *q == p.cutoff && item.allowed;
            }
            (None, Some(g)) => {
                if g.quality_ids.is_empty() {
                    return Err(DbError::Invalid(format!("group `{}` is empty", g.name)));
                }
                for q in &g.quality_ids {
                    if Quality::by_id(*q).is_none() || !seen.insert(*q) {
                        return Err(DbError::Invalid(format!(
                            "invalid or duplicate quality {q} in group `{}`",
                            g.name
                        )));
                    }
                }
                cutoff_found |= g.id == p.cutoff && item.allowed;
            }
            _ => {
                return Err(DbError::Invalid(
                    "each item must be a quality or a group".into(),
                ));
            }
        }
        any_allowed |= item.allowed;
    }
    for (codec, factor) in &p.codec_multipliers {
        if !kitsunarr_domain::codec::FAMILIES.contains(&codec.as_str()) {
            return Err(DbError::Invalid(format!("unknown codec family {codec}")));
        }
        if !(0.01..=10.0).contains(&factor.0) {
            return Err(DbError::Invalid(format!(
                "the {codec} factor must be between 0.01 and 10"
            )));
        }
    }
    let mut limited = std::collections::HashSet::new();
    for l in &p.size_limits {
        if Quality::by_id(l.quality_id).is_none() || !limited.insert(l.quality_id) {
            return Err(DbError::Invalid(format!(
                "invalid or duplicate size limit for quality {}",
                l.quality_id
            )));
        }
        let v = |o: Option<kitsunarr_domain::api::settings::OrderedF64>| o.map(|x| x.0);
        let (min, preferred, max) = (v(l.min_size), v(l.preferred_size), v(l.max_size));
        if [min, preferred, max]
            .into_iter()
            .flatten()
            .any(|x| !x.is_finite() || !(0.0..=1000.0).contains(&x))
        {
            return Err(DbError::Invalid(
                "sizes must be between 0 and 1000 MiB per minute".into(),
            ));
        }
        let max = max.filter(|m| *m > 0.0);
        if min.zip(preferred).is_some_and(|(a, b)| a > b)
            || max.is_some_and(|m| min.is_some_and(|a| a > m) || preferred.is_some_and(|b| b > m))
        {
            return Err(DbError::Invalid(format!(
                "quality {}: minimum ≤ preferred ≤ maximum",
                l.quality_id
            )));
        }
    }
    if !any_allowed {
        return Err(DbError::Invalid(
            "at least one quality must be allowed".into(),
        ));
    }
    if !cutoff_found {
        return Err(DbError::Invalid(
            "cutoff must be an allowed quality or group".into(),
        ));
    }
    Ok(())
}

pub async fn create_profile(ex: impl SqliteExecutor<'_>, p: &QualityProfile) -> DbResult<i64> {
    validate_profile(p)?;
    sqlx::query_scalar(
        "INSERT INTO quality_profiles (name, upgrade_allowed, cutoff, items, min_format_score, cutoff_format_score, \
         min_upgrade_format_score, format_items, size_limits, codec_multipliers) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(p.name.trim())
    .bind(p.upgrade_allowed)
    .bind(p.cutoff)
    .bind(to_json(&p.items)?)
    .bind(p.min_format_score)
    .bind(p.cutoff_format_score)
    .bind(p.min_upgrade_format_score)
    .bind(to_json(&p.format_items)?)
    .bind(to_json(&p.size_limits)?)
    .bind(to_json(&p.codec_multipliers)?)
    .fetch_one(ex)
    .await
    .map_err(|e| DbError::from_write(e, "quality profile"))
}

pub async fn update_profile(ex: impl SqliteExecutor<'_>, p: &QualityProfile) -> DbResult<()> {
    validate_profile(p)?;
    let r = sqlx::query(
        "UPDATE quality_profiles SET name = ?, upgrade_allowed = ?, cutoff = ?, items = ?, min_format_score = ?, \
         cutoff_format_score = ?, min_upgrade_format_score = ?, format_items = ?, size_limits = ?, \
         codec_multipliers = ? WHERE id = ?",
    )
    .bind(p.name.trim())
    .bind(p.upgrade_allowed)
    .bind(p.cutoff)
    .bind(to_json(&p.items)?)
    .bind(p.min_format_score)
    .bind(p.cutoff_format_score)
    .bind(p.min_upgrade_format_score)
    .bind(to_json(&p.format_items)?)
    .bind(to_json(&p.size_limits)?)
    .bind(to_json(&p.codec_multipliers)?)
    .bind(p.id)
    .execute(ex)
    .await
    .map_err(|e| DbError::from_write(e, "quality profile"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn delete_profile(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<()> {
    let r = sqlx::query("DELETE FROM quality_profiles WHERE id = ?")
        .bind(id)
        .execute(ex)
        .await
        .map_err(|e| DbError::from_write(e, "quality profile in use by series or movies;"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

fn q(id: i32, allowed: bool) -> QualityProfileItem {
    QualityProfileItem {
        quality_id: Some(id),
        group: None,
        allowed,
    }
}

fn g(id: i32, name: &str, ids: &[i32], allowed: bool) -> QualityProfileItem {
    QualityProfileItem {
        quality_id: None,
        group: Some(QualityGroup {
            id,
            name: name.into(),
            quality_ids: ids.to_vec(),
        }),
        allowed,
    }
}

/// Default item list in Sonarr's order (worst → best) with the given entries allowed.
/// Group ids: 1000 WEB 480p, 1001 WEB 720p, 1002 WEB 1080p, 1003 WEB 2160p.
pub fn default_items(allowed: &[i32]) -> Vec<QualityProfileItem> {
    let a = |id: i32| allowed.contains(&id);
    vec![
        q(0, a(0)),
        q(1, a(1)),
        g(1000, "WEB 480p", &[12, 8], a(1000)),
        q(2, a(2)),
        q(13, a(13)),
        q(22, a(22)),
        q(4, a(4)),
        q(9, a(9)),
        q(10, a(10)),
        g(1001, "WEB 720p", &[14, 5], a(1001)),
        q(6, a(6)),
        g(1002, "WEB 1080p", &[15, 3], a(1002)),
        q(7, a(7)),
        q(20, a(20)),
        q(16, a(16)),
        g(1003, "WEB 2160p", &[17, 18], a(1003)),
        q(19, a(19)),
        q(21, a(21)),
    ]
}

/// Sonarr v4's default profiles (name, cutoff, allowed entries).
pub fn default_profiles() -> Vec<QualityProfile> {
    let make = |name: &str, cutoff: i32, allowed: &[i32]| QualityProfile {
        id: 0,
        name: name.into(),
        upgrade_allowed: false,
        cutoff,
        items: default_items(allowed),
        min_format_score: 0,
        cutoff_format_score: 0,
        min_upgrade_format_score: 1,
        format_items: Vec::new(),
        size_limits: Vec::new(),
        codec_multipliers: Default::default(),
    };
    vec![
        make("Any", 1, &[1, 1000, 2, 13, 22, 4, 9, 1001, 6, 1002, 7]),
        make("SD", 1, &[1, 1000, 2, 13, 22]),
        make("HD-720p", 4, &[4, 1001, 6]),
        make("HD-1080p", 9, &[9, 1002, 7]),
        make("Ultra-HD", 16, &[16, 1003, 19]),
        make("HD - 720p/1080p", 4, &[4, 9, 1001, 6, 1002, 7]),
    ]
}

/// Default definitions from the catalogue.
pub fn default_definitions() -> Vec<QualityDefinition> {
    QUALITIES
        .iter()
        .map(|q| QualityDefinition {
            quality_id: q.id,
            title: q.name.into(),
            weight: q.weight,
            min_size: Some(OrderedF64(q.default_min_size)),
            preferred_size: Some(OrderedF64(q.default_preferred_size)),
            max_size: q.default_max_size.map(OrderedF64),
            ..Default::default()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Custom formats
// ---------------------------------------------------------------------------------------------

#[derive(FromRow)]
struct FormatRow {
    id: i64,
    name: String,
    include_when_renaming: bool,
    specifications: String,
}

pub async fn list_custom_formats(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<CustomFormat>> {
    let rows: Vec<FormatRow> =
        sqlx::query_as("SELECT * FROM custom_formats ORDER BY name COLLATE NOCASE")
            .fetch_all(ex)
            .await?;
    rows.into_iter()
        .map(|r| {
            Ok(CustomFormat {
                id: r.id,
                name: r.name,
                include_when_renaming: r.include_when_renaming,
                specifications: from_json::<Vec<CustomFormatSpecification>>(&r.specifications)?,
            })
        })
        .collect()
}

pub fn validate_custom_format(cf: &CustomFormat) -> Result<(), DbError> {
    if cf.name.trim().is_empty() {
        return Err(DbError::Invalid("custom format name is required".into()));
    }
    if cf.specifications.is_empty() {
        return Err(DbError::Invalid(
            "a custom format needs at least one specification".into(),
        ));
    }
    for spec in &cf.specifications {
        if spec.implementation == "ReleaseTitleSpecification"
            || spec.implementation == "ReleaseGroupSpecification"
        {
            let pattern = spec
                .fields
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            regex_syntax_check(pattern)
                .map_err(|e| DbError::Invalid(format!("`{}`: {e}", spec.name)))?;
        }
    }
    Ok(())
}

/// Minimal sanity check so obviously broken patterns are rejected at save time; full regex
/// compilation (with .NET-compatibility translation) happens in the release-matching engine.
fn regex_syntax_check(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("pattern is empty".into());
    }
    let mut depth = 0i32;
    let mut escaped = false;
    for c in pattern.chars() {
        match (escaped, c) {
            (true, _) => escaped = false,
            (false, '\\') => escaped = true,
            (false, '(') => depth += 1,
            (false, ')') => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return Err("unbalanced parentheses".into());
        }
    }
    if depth != 0 {
        Err("unbalanced parentheses".into())
    } else {
        Ok(())
    }
}

pub async fn create_custom_format(ex: impl SqliteExecutor<'_>, cf: &CustomFormat) -> DbResult<i64> {
    validate_custom_format(cf)?;
    sqlx::query_scalar(
        "INSERT INTO custom_formats (name, include_when_renaming, specifications) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(cf.name.trim())
    .bind(cf.include_when_renaming)
    .bind(to_json(&cf.specifications)?)
    .fetch_one(ex)
    .await
    .map_err(|e| DbError::from_write(e, "custom format"))
}

pub async fn update_custom_format(ex: impl SqliteExecutor<'_>, cf: &CustomFormat) -> DbResult<()> {
    validate_custom_format(cf)?;
    let r = sqlx::query("UPDATE custom_formats SET name = ?, include_when_renaming = ?, specifications = ? WHERE id = ?")
        .bind(cf.name.trim())
        .bind(cf.include_when_renaming)
        .bind(to_json(&cf.specifications)?)
        .bind(cf.id)
        .execute(ex)
        .await
        .map_err(|e| DbError::from_write(e, "custom format"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Deletes a format and removes its score entries from every profile.
pub async fn delete_custom_format(db: &crate::Database, id: i64) -> DbResult<()> {
    let mut tx = db.pool().begin().await?;
    let r = sqlx::query("DELETE FROM custom_formats WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, format_items FROM quality_profiles")
        .fetch_all(&mut *tx)
        .await?;
    for (pid, items) in rows {
        let mut scores: Vec<FormatScore> = from_json(&items)?;
        let before = scores.len();
        scores.retain(|s| s.format_id != id);
        if scores.len() != before {
            sqlx::query("UPDATE quality_profiles SET format_items = ? WHERE id = ?")
                .bind(to_json(&scores)?)
                .bind(pid)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profiles_are_valid() {
        for p in default_profiles() {
            validate_profile(&p).unwrap_or_else(|e| panic!("{}: {e}", p.name));
        }
        let items = default_items(&[]);
        let covered: usize = items
            .iter()
            .map(|i| i.group.as_ref().map_or(1, |g| g.quality_ids.len()))
            .sum();
        assert_eq!(
            covered,
            QUALITIES.len(),
            "every catalogue quality appears exactly once"
        );
    }

    #[test]
    fn profile_validation_rejects_bad_cutoff() {
        let mut p = default_profiles().remove(3);
        p.cutoff = 21; // not allowed in HD-1080p
        assert!(validate_profile(&p).is_err());
    }

    #[tokio::test]
    async fn seeded_profiles_and_format_cleanup() {
        let db = crate::test_support::db().await;
        let profiles = list_profiles(db.pool()).await.unwrap();
        assert_eq!(profiles.len(), 6);
        assert_eq!(
            list_definitions(db.pool()).await.unwrap().len(),
            QUALITIES.len()
        );

        let cf = CustomFormat {
            id: 0,
            name: "x265".into(),
            include_when_renaming: true,
            specifications: vec![CustomFormatSpecification {
                name: "x265".into(),
                implementation: "ReleaseTitleSpecification".into(),
                negate: false,
                required: true,
                fields: serde_json::json!({"value": "[xh][ ._-]?265"})
                    .as_object()
                    .unwrap()
                    .clone(),
            }],
        };
        let id = create_custom_format(db.pool(), &cf).await.unwrap();
        let mut p = profiles[0].clone();
        p.format_items = vec![FormatScore {
            format_id: id,
            score: 100,
        }];
        update_profile(db.pool(), &p).await.unwrap();
        delete_custom_format(&db, id).await.unwrap();
        assert!(
            get_profile(db.pool(), p.id)
                .await
                .unwrap()
                .format_items
                .is_empty()
        );

        let mut broken = cf.clone();
        broken.specifications[0]
            .fields
            .insert("value".into(), serde_json::json!("(unclosed"));
        assert!(create_custom_format(db.pool(), &broken).await.is_err());
    }
}
