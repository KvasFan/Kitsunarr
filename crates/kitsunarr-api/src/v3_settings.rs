//! Sonarr's `/api/v3` settings resources that sync tools write (Recyclarr, Notifiarr's
//! TRaSH sync, Profilarr): custom formats, quality definitions, quality profiles, release
//! profiles and languages, in Sonarr's JSON shapes and mapped onto Kitsunarr's settings.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use kitsunarr_db::quality;
use kitsunarr_domain::api::settings::{
    CustomFormat, CustomFormatSpecification, FormatScore, OrderedF64, QualityDefinition,
    QualityGroup, QualityProfile, QualityProfileItem, QualitySizeLimit, ReleaseProfile,
};
use kitsunarr_domain::language::Language;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::{rules, settings};
use crate::v3::quality_json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/customformat", get(list_formats).post(create_format))
        .route("/customformat/schema", get(format_schema))
        .route(
            "/customformat/{id}",
            get(get_format).put(update_format).delete(delete_format),
        )
        .route("/qualitydefinition", get(list_definitions))
        .route("/qualitydefinition/update", put(update_definitions))
        .route(
            "/qualitydefinition/{id}",
            get(get_definition).put(update_definition),
        )
        .route("/qualityprofile/schema", get(profile_schema))
        .route("/qualityprofile", axum::routing::post(create_profile))
        .route(
            "/qualityprofile/{id}",
            get(get_profile).put(update_profile).delete(delete_profile),
        )
        .route(
            "/releaseprofile",
            get(list_release_profiles).post(create_release_profile),
        )
        .route(
            "/releaseprofile/{id}",
            get(get_release_profile)
                .put(update_release_profile)
                .delete(delete_release_profile),
        )
        .route("/language", get(languages))
}

// ---- custom formats ------------------------------------------------------------------------

/// Sonarr's specifications and the fields each has (`name`, `label`, `type`).
/// (name, label, type) of a specification field.
pub(crate) type Field = (&'static str, &'static str, &'static str);

pub(crate) const SPECIFICATIONS: [(&str, &str, &[Field]); 8] = [
    (
        "ReleaseTitleSpecification",
        "Release Title",
        &[("value", "Regular Expression", "textbox")],
    ),
    (
        "LanguageSpecification",
        "Language",
        &[
            ("value", "Language", "select"),
            ("exceptLanguage", "Except Language", "checkbox"),
        ],
    ),
    (
        "IndexerFlagSpecification",
        "Indexer Flag",
        &[("value", "Flag", "select")],
    ),
    (
        "SourceSpecification",
        "Source",
        &[("value", "Source", "select")],
    ),
    (
        "ResolutionSpecification",
        "Resolution",
        &[("value", "Resolution", "select")],
    ),
    (
        "ReleaseGroupSpecification",
        "Release Group",
        &[("value", "Regular Expression", "textbox")],
    ),
    (
        "SizeSpecification",
        "Size",
        &[
            ("min", "Minimum Size", "number"),
            ("max", "Maximum Size", "number"),
        ],
    ),
    (
        "ReleaseTypeSpecification",
        "Release Type",
        &[("value", "Release Type", "select")],
    ),
];

pub(crate) fn field_json(order: usize, name: &str, label: &str, kind: &str, value: Value) -> Value {
    json!({"order": order, "name": name, "label": label, "value": value, "type": kind, "advanced": false, "privacy": "normal", "isFloat": kind == "number"})
}

pub(crate) fn spec_json(s: &CustomFormatSpecification) -> Value {
    let known = SPECIFICATIONS
        .iter()
        .find(|(i, _, _)| *i == s.implementation);
    let mut fields: Vec<Value> = Vec::new();
    if let Some((_, _, defs)) = known {
        for (n, (name, label, kind)) in defs.iter().enumerate() {
            let value = s.fields.get(*name).cloned().unwrap_or(Value::Null);
            fields.push(field_json(n, name, label, kind, value));
        }
    }
    for (name, value) in &s.fields {
        if !known.is_some_and(|(_, _, defs)| defs.iter().any(|(n, _, _)| n == name)) {
            fields.push(field_json(
                fields.len(),
                name,
                name,
                "textbox",
                value.clone(),
            ));
        }
    }
    json!({
        "name": s.name,
        "implementation": s.implementation,
        "implementationName": known.map_or(s.implementation.as_str(), |(_, n, _)| n),
        "infoLink": "https://wiki.servarr.com/sonarr/settings#custom-formats-2",
        "negate": s.negate,
        "required": s.required,
        "fields": fields,
    })
}

pub(crate) fn format_json(f: &CustomFormat) -> Value {
    json!({
        "id": f.id,
        "name": f.name,
        "includeCustomFormatWhenRenaming": f.include_when_renaming,
        "specifications": f.specifications.iter().map(spec_json).collect::<Vec<_>>(),
    })
}

/// A `CustomFormatResource` as Kitsunarr's custom format. Fields come as Sonarr's list of
/// `{name, value}` (or already as an object).
pub(crate) fn format_from(v: &Value, id: i64) -> ApiResult<CustomFormat> {
    let name = v["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| ApiError::field("name", "The custom format needs a name."))?;
    let mut specifications = Vec::new();
    for s in v["specifications"].as_array().into_iter().flatten() {
        let mut fields = Map::new();
        match &s["fields"] {
            Value::Array(list) => {
                for f in list {
                    if let Some(n) = f["name"].as_str() {
                        fields.insert(n.to_string(), f["value"].clone());
                    }
                }
            }
            Value::Object(map) => fields = map.clone(),
            _ => {}
        }
        specifications.push(CustomFormatSpecification {
            name: s["name"].as_str().unwrap_or_default().to_string(),
            implementation: s["implementation"]
                .as_str()
                .ok_or_else(|| {
                    ApiError::field(
                        "specifications",
                        "Each specification needs an implementation.",
                    )
                })?
                .to_string(),
            negate: s["negate"].as_bool().unwrap_or(false),
            required: s["required"].as_bool().unwrap_or(false),
            fields,
        });
    }
    Ok(CustomFormat {
        id,
        name: name.trim().to_string(),
        include_when_renaming: v["includeCustomFormatWhenRenaming"]
            .as_bool()
            .unwrap_or(false),
        specifications,
    })
}

pub(crate) async fn list_formats(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    let formats = quality::list_custom_formats(state.db.pool()).await?;
    Ok(Json(formats.iter().map(format_json).collect()))
}

pub(crate) async fn find_format(state: &AppState, id: i64) -> ApiResult<CustomFormat> {
    quality::list_custom_formats(state.db.pool())
        .await?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(|| ApiError::NotFound("The custom format does not exist.".into()))
}

pub(crate) async fn get_format(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(format_json(&find_format(&state, id).await?)))
}

pub(crate) async fn create_format(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let cf = format_from(&v, 0)?;
    let (status, Json(created)) = settings::create_custom_format(State(state), Json(cf)).await?;
    Ok((status, Json(format_json(&created))))
}

pub(crate) async fn update_format(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    find_format(&state, id).await?;
    let cf = format_from(&v, id)?;
    let Json(updated) = settings::update_custom_format(State(state), Path(id), Json(cf)).await?;
    Ok((StatusCode::ACCEPTED, Json(format_json(&updated))))
}

pub(crate) async fn delete_format(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    find_format(&state, id).await?;
    settings::delete_custom_format(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

/// `GET /customformat/schema`: every specification with empty fields.
pub(crate) async fn format_schema() -> Json<Vec<Value>> {
    Json(
        SPECIFICATIONS
            .iter()
            .map(|(implementation, _, _)| {
                spec_json(&CustomFormatSpecification {
                    name: String::new(),
                    implementation: (*implementation).to_string(),
                    negate: false,
                    required: false,
                    fields: Map::new(),
                })
            })
            .collect(),
    )
}

// ---- quality definitions -------------------------------------------------------------------

/// Tanukarr's `QualityProfileSizeLimit.Summarize` for one quality: each size (min, preferred,
/// max) across the profiles (a profile without its own limit uses the definition's), `None`
/// with the mixed flag when they differ.
pub(crate) struct SizeSummary {
    pub(crate) sizes: [Option<f64>; 3],
    pub(crate) mixed: [bool; 3],
}

pub(crate) fn size_summary(d: &QualityDefinition, profiles: &[QualityProfile]) -> SizeSummary {
    let own = [d.min_size, d.preferred_size, d.max_size].map(|s| s.map(|v| v.0));
    if profiles.is_empty() {
        return SizeSummary {
            sizes: own,
            mixed: [false; 3],
        };
    }
    let mut sizes = [None; 3];
    let mut mixed = [false; 3];
    for slot in 0..3 {
        let mut values: Vec<Option<f64>> = Vec::new();
        for p in profiles {
            let v = p
                .size_limits
                .iter()
                .find(|l| l.quality_id == d.quality_id)
                .map_or(own[slot], |l| {
                    [l.min_size, l.preferred_size, l.max_size][slot].map(|v| v.0)
                });
            // A maximum of 0 is no limit.
            let v = if slot == 2 && v == Some(0.0) { None } else { v };
            if !values.contains(&v) {
                values.push(v);
            }
        }
        if values.len() == 1 {
            sizes[slot] = values[0];
        } else {
            mixed[slot] = true;
        }
    }
    SizeSummary { sizes, mixed }
}

/// Tanukarr's `QualityDefinitionResource` (criteria, retirement, sizes summarised across the
/// profiles with their mixed flags).
pub(crate) fn definition_json(d: &QualityDefinition, profiles: &[QualityProfile]) -> Value {
    let built_in = kitsunarr_domain::quality::Quality::by_id(d.quality_id);
    let summary = size_summary(d, profiles);
    let source = d.source.or(built_in.map(|q| q.source));
    json!({
        "id": d.quality_id,
        "quality": quality_json(d.quality_id),
        "title": d.title,
        "weight": d.weight,
        "source": source.map(|s| serde_json::to_value(s).unwrap_or_default()),
        "resolution": d.resolution.or(built_in.map(|q| q.resolution)),
        "codec": d.codec,
        "titlePattern": d.title_pattern,
        "isRetired": d.is_retired,
        "isCustom": d.quality_id >= kitsunarr_domain::quality::CUSTOM_QUALITY_BASE,
        "minSize": summary.sizes[0],
        "maxSize": summary.sizes[2],
        "preferredSize": summary.sizes[1],
        "minSizeIsMixed": summary.mixed[0],
        "maxSizeIsMixed": summary.mixed[2],
        "preferredSizeIsMixed": summary.mixed[1],
    })
}

pub(crate) async fn definitions_json(state: &AppState) -> ApiResult<Vec<Value>> {
    let defs = quality::list_definitions(state.db.pool()).await?;
    let profiles = quality::list_profiles(state.db.pool()).await?;
    Ok(defs.iter().map(|d| definition_json(d, &profiles)).collect())
}

pub(crate) async fn list_definitions(State(state): State<AppState>) -> ApiResult<Json<Vec<Value>>> {
    Ok(Json(definitions_json(&state).await?))
}

pub(crate) async fn get_definition(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<Value>> {
    definitions_json(&state)
        .await?
        .into_iter()
        .find(|d| d["id"] == json!(id))
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("The quality definition does not exist.".into()))
}

/// Applies a `QualityDefinitionResource` (title and sizes) to the stored definition. As in
/// Sonarr, the resource replaces all three sizes: a size that is missing or null is no limit
/// (Recyclarr leaves out the null ones, e.g. TRaSH's unlimited maximums).
pub(crate) fn apply_definition(d: &mut QualityDefinition, v: &Value) {
    let num = |key: &str| v[key].as_f64().map(OrderedF64);
    if let Some(t) = v["title"].as_str().filter(|t| !t.trim().is_empty()) {
        d.title = t.to_string();
    }
    d.min_size = num("minSize");
    d.max_size = num("maxSize");
    d.preferred_size = num("preferredSize");
}

pub(crate) fn definition_id(v: &Value) -> Option<i32> {
    v["quality"]["id"]
        .as_i64()
        .or_else(|| v["id"].as_i64())
        .and_then(|i| i32::try_from(i).ok())
}

pub(crate) async fn save_definitions(state: &AppState, changes: &[Value]) -> ApiResult<Vec<Value>> {
    let mut defs = quality::list_definitions(state.db.pool()).await?;
    for v in changes {
        let id = definition_id(v)
            .ok_or_else(|| ApiError::field("quality", "Each definition needs its quality."))?;
        let d = defs
            .iter_mut()
            .find(|d| d.quality_id == id)
            .ok_or_else(|| ApiError::NotFound(format!("Quality {id} does not exist.")))?;
        apply_definition(d, v);
    }
    let Json(saved) =
        settings::update_quality_definitions(State(state.clone()), Json(defs)).await?;
    let ids: Vec<i32> = changes.iter().filter_map(definition_id).collect();
    let profiles = quality::list_profiles(state.db.pool()).await?;
    Ok(saved
        .iter()
        .filter(|d| ids.contains(&d.quality_id))
        .map(|d| definition_json(d, &profiles))
        .collect())
}

pub(crate) async fn update_definition(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(mut v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    v["quality"]["id"] = json!(id);
    let saved = save_definitions(&state, std::slice::from_ref(&v)).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(saved.into_iter().next().unwrap_or(Value::Null)),
    ))
}

/// `PUT /qualitydefinition/update`: several definitions at once (Recyclarr's size sync).
pub(crate) async fn update_definitions(
    State(state): State<AppState>,
    Json(v): Json<Vec<Value>>,
) -> ApiResult<(StatusCode, Json<Vec<Value>>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(save_definitions(&state, &v).await?),
    ))
}

// ---- quality profiles ----------------------------------------------------------------------

/// Sonarr's `QualityProfileResource`, every custom format listed in `formatItems`.
pub(crate) fn profile_json(p: &QualityProfile, formats: &[CustomFormat]) -> Value {
    // Tanukarr's QualityProfileResource stores size limits on individual quality leaves.
    let leaf = |q: i32, allowed: bool| {
        let mut v = json!({"quality": quality_json(q), "items": [], "allowed": allowed});
        if let Some(l) = p.size_limits.iter().find(|l| l.quality_id == q) {
            v["minSize"] = json!(l.min_size);
            v["preferredSize"] = json!(l.preferred_size);
            v["maxSize"] = json!(l.max_size);
        }
        v
    };
    let items: Vec<Value> = p
        .items
        .iter()
        .map(|i| match (&i.quality_id, &i.group) {
            (Some(q), _) => leaf(*q, i.allowed),
            (None, Some(g)) => json!({
                "id": g.id, "name": g.name, "allowed": i.allowed,
                "items": g.quality_ids.iter().map(|q| leaf(*q, i.allowed)).collect::<Vec<_>>(),
            }),
            _ => json!({}),
        })
        .collect();
    let format_items: Vec<Value> = formats
        .iter()
        .map(|f| {
            let score = p
                .format_items
                .iter()
                .find(|s| s.format_id == f.id)
                .map_or(0, |s| s.score);
            json!({"format": f.id, "name": f.name, "score": score})
        })
        .collect();
    json!({
        "id": p.id,
        "name": p.name,
        "upgradeAllowed": p.upgrade_allowed,
        "cutoff": p.cutoff,
        "items": items,
        "minFormatScore": p.min_format_score,
        "cutoffFormatScore": p.cutoff_format_score,
        "minUpgradeFormatScore": p.min_upgrade_format_score,
        "formatItems": format_items,
        "codecMultipliers": p.codec_multipliers,
        "language": {"id": 1, "name": "English"},
    })
}

/// A Sonarr/Tanukarr `QualityProfileResource` over `base`. Omitted fields are preserved.
pub(crate) fn profile_from(v: &Value, mut base: QualityProfile) -> ApiResult<QualityProfile> {
    if let Some(value) = v.get("codecMultipliers") {
        base.codec_multipliers = if value.is_null() {
            Default::default()
        } else {
            serde_json::from_value(value.clone())
                .map_err(|_| ApiError::field("codecMultipliers", "Expected codec factors."))?
        };
    }
    if let Some(name) = v["name"].as_str() {
        base.name = name.trim().to_string();
    }
    if let Some(u) = v["upgradeAllowed"].as_bool() {
        base.upgrade_allowed = u;
    }
    if let Some(c) = v["cutoff"].as_i64() {
        base.cutoff = c as i32;
    }
    let int = |key: &str| v[key].as_i64().map(|n| n as i32);
    if let Some(n) = int("minFormatScore") {
        base.min_format_score = n;
    }
    if let Some(n) = int("cutoffFormatScore") {
        base.cutoff_format_score = n;
    }
    if let Some(n) = int("minUpgradeFormatScore") {
        base.min_upgrade_format_score = n;
    }
    if let Some(items) = v["items"].as_array() {
        let mut out = Vec::new();
        for i in items {
            let allowed = i["allowed"].as_bool().unwrap_or(false);
            let children = i["items"].as_array().cloned().unwrap_or_default();
            if let Some(q) = i["quality"]["id"].as_i64() {
                apply_profile_sizes(&mut base, q as i32, i)?;
                out.push(QualityProfileItem {
                    quality_id: Some(q as i32),
                    group: None,
                    allowed,
                });
            } else if !children.is_empty() {
                for child in &children {
                    if let Some(q) = child["quality"]["id"].as_i64() {
                        apply_profile_sizes(&mut base, q as i32, child)?;
                    }
                }
                let quality_ids: Vec<i32> = children
                    .iter()
                    .filter_map(|c| c["quality"]["id"].as_i64().map(|q| q as i32))
                    .collect();
                out.push(QualityProfileItem {
                    quality_id: None,
                    group: Some(QualityGroup {
                        id: i["id"]
                            .as_i64()
                            .map_or(1000 + out.len() as i32, |n| n as i32),
                        name: i["name"].as_str().unwrap_or("Group").to_string(),
                        quality_ids,
                    }),
                    allowed,
                });
            } else {
                return Err(ApiError::field(
                    "items",
                    "Each profile item needs a quality or a group of qualities.",
                ));
            }
        }
        base.items = out;
    }
    if let Some(items) = v["formatItems"].as_array() {
        base.format_items = items
            .iter()
            .filter_map(|f| {
                Some(FormatScore {
                    format_id: f["format"].as_i64()?,
                    score: f["score"].as_i64().unwrap_or(0) as i32,
                })
            })
            .filter(|f| f.score != 0)
            .collect();
    }
    if base.name.is_empty() {
        return Err(ApiError::field("name", "The profile needs a name."));
    }
    Ok(base)
}

pub(crate) fn apply_profile_sizes(
    p: &mut QualityProfile,
    quality_id: i32,
    v: &Value,
) -> ApiResult<()> {
    let fields = ["minSize", "preferredSize", "maxSize"];
    if !fields.iter().any(|f| v.get(*f).is_some()) {
        return Ok(());
    }
    let mut limit = p
        .size_limits
        .iter()
        .find(|l| l.quality_id == quality_id)
        .cloned()
        .unwrap_or(QualitySizeLimit {
            quality_id,
            min_size: None,
            preferred_size: None,
            max_size: None,
        });
    for (field, slot) in fields.into_iter().zip([
        &mut limit.min_size,
        &mut limit.preferred_size,
        &mut limit.max_size,
    ]) {
        if let Some(value) = v.get(field) {
            *slot = if value.is_null() {
                None
            } else {
                Some(OrderedF64(value.as_f64().ok_or_else(|| {
                    ApiError::field(field, "Expected a size in MiB per minute or null.")
                })?))
            };
        }
    }
    p.size_limits.retain(|l| l.quality_id != quality_id);
    p.size_limits.push(limit);
    Ok(())
}

pub(crate) async fn get_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let p = quality::get_profile(state.db.pool(), id).await?;
    let formats = quality::list_custom_formats(state.db.pool()).await?;
    Ok(Json(profile_json(&p, &formats)))
}

/// `GET /qualityprofile/schema`: a new profile with every quality, none allowed.
pub(crate) async fn profile_schema(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let defs = quality::list_definitions(state.db.pool()).await?;
    let formats = quality::list_custom_formats(state.db.pool()).await?;
    let mut defs = defs;
    defs.sort_by_key(|d| d.weight);
    let profile = QualityProfile {
        id: 0,
        name: String::new(),
        upgrade_allowed: false,
        cutoff: 0,
        items: defs
            .iter()
            .map(|d| QualityProfileItem {
                quality_id: Some(d.quality_id),
                group: None,
                allowed: false,
            })
            .collect(),
        min_format_score: 0,
        cutoff_format_score: 0,
        min_upgrade_format_score: 1,
        format_items: Vec::new(),
        size_limits: Vec::new(),
        codec_multipliers: Default::default(),
    };
    Ok(Json(profile_json(&profile, &formats)))
}

pub(crate) async fn create_profile(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let Json(schema) = profile_schema(State(state.clone())).await?;
    let base: QualityProfile =
        profile_from(&schema, empty_profile()).unwrap_or_else(|_| empty_profile());
    let p = profile_from(&v, base)?;
    let (status, Json(created)) =
        settings::create_quality_profile(State(state.clone()), Json(p)).await?;
    let formats = quality::list_custom_formats(state.db.pool()).await?;
    Ok((status, Json(profile_json(&created, &formats))))
}

pub(crate) fn empty_profile() -> QualityProfile {
    QualityProfile {
        id: 0,
        name: String::new(),
        upgrade_allowed: false,
        cutoff: 0,
        items: Vec::new(),
        min_format_score: 0,
        cutoff_format_score: 0,
        min_upgrade_format_score: 1,
        format_items: Vec::new(),
        size_limits: Vec::new(),
        codec_multipliers: Default::default(),
    }
}

pub(crate) async fn update_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let base = quality::get_profile(state.db.pool(), id).await?;
    let p = profile_from(&v, base)?;
    let Json(updated) =
        settings::update_quality_profile(State(state.clone()), Path(id), Json(p)).await?;
    let formats = quality::list_custom_formats(state.db.pool()).await?;
    Ok((StatusCode::ACCEPTED, Json(profile_json(&updated, &formats))))
}

pub(crate) async fn delete_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    quality::get_profile(state.db.pool(), id).await?;
    settings::delete_quality_profile(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

// ---- release profiles ----------------------------------------------------------------------

pub(crate) fn release_profile_json(r: &ReleaseProfile) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "enabled": r.enabled,
        "required": r.required,
        "ignored": r.ignored,
        "indexerId": r.indexer_ids.first().copied().unwrap_or(0),
        "tags": r.tags,
    })
}

/// Terms as Sonarr v4 takes them: a list, or one comma-separated string.
pub(crate) fn terms(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect(),
        Value::String(s) => s
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn release_profile_from(v: &Value, id: i64) -> ReleaseProfile {
    ReleaseProfile {
        id,
        name: v["name"].as_str().map(str::to_string),
        enabled: v["enabled"].as_bool().unwrap_or(true),
        required: terms(&v["required"]),
        ignored: terms(&v["ignored"]),
        indexer_ids: v["indexerId"]
            .as_i64()
            .filter(|i| *i > 0)
            .into_iter()
            .collect(),
        tags: v["tags"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
    }
}

pub(crate) async fn list_release_profiles(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Value>>> {
    let Json(list) = rules::list_release(State(state)).await?;
    Ok(Json(list.iter().map(release_profile_json).collect()))
}

pub(crate) async fn find_release_profile(state: &AppState, id: i64) -> ApiResult<ReleaseProfile> {
    let Json(list) = rules::list_release(State(state.clone())).await?;
    list.into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::NotFound("The release profile does not exist.".into()))
}

pub(crate) async fn get_release_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(release_profile_json(
        &find_release_profile(&state, id).await?,
    )))
}

pub(crate) async fn create_release_profile(
    State(state): State<AppState>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (status, Json(created)) =
        rules::create_release(State(state), Json(release_profile_from(&v, 0))).await?;
    Ok((status, Json(release_profile_json(&created))))
}

pub(crate) async fn update_release_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    find_release_profile(&state, id).await?;
    let Json(updated) =
        rules::update_release(State(state), Path(id), Json(release_profile_from(&v, id))).await?;
    Ok((StatusCode::ACCEPTED, Json(release_profile_json(&updated))))
}

pub(crate) async fn delete_release_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    find_release_profile(&state, id).await?;
    rules::delete_release(State(state), Path(id)).await?;
    Ok(StatusCode::OK)
}

// ---- languages -----------------------------------------------------------------------------

/// `GET /language`: Sonarr's languages.
pub(crate) async fn languages() -> Json<Vec<Value>> {
    Json(
        Language::all()
            .iter()
            .map(|l| json!({"id": l.id, "name": l.name}))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_formats_round_trip_through_sonarr_fields() {
        let v = json!({
            "name": "x265 (HD)",
            "includeCustomFormatWhenRenaming": true,
            "specifications": [
                {"name": "x265", "implementation": "ReleaseTitleSpecification", "negate": false, "required": true,
                 "fields": [{"name": "value", "value": "[xh][ ._-]?265|\\bHEVC(\\b|\\d)"}]},
                {"name": "Not 2160p", "implementation": "ResolutionSpecification", "negate": true, "required": false,
                 "fields": [{"name": "value", "value": 2160}]},
            ]
        });
        let cf = format_from(&v, 7).unwrap();
        assert_eq!(cf.id, 7);
        assert!(cf.include_when_renaming);
        assert_eq!(
            cf.specifications[0].fields["value"],
            json!("[xh][ ._-]?265|\\bHEVC(\\b|\\d)")
        );
        assert!(cf.specifications[1].negate);
        let back = format_json(&cf);
        assert_eq!(back["specifications"][1]["fields"][0]["name"], "value");
        assert_eq!(back["specifications"][1]["fields"][0]["value"], 2160);
        assert_eq!(
            back["specifications"][1]["implementationName"],
            "Resolution"
        );
        assert!(format_from(&json!({"name": " "}), 0).is_err());
    }

    #[test]
    fn release_profile_terms_take_lists_or_strings() {
        let r = release_profile_from(
            &json!({"name": "x", "required": "a, b,", "ignored": ["c", " "], "indexerId": 0, "tags": [3]}),
            0,
        );
        assert_eq!(r.required, ["a", "b"]);
        assert_eq!(r.ignored, ["c"]);
        assert!(r.indexer_ids.is_empty());
        assert_eq!(r.tags, [3]);
    }
}
