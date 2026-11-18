//! Native endpoints behind Tanukarr's interface pages: naming samples, library statistics,
//! collection settings, host settings, log files and the parse tester.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use kitsunarr_db::library::{collections, series};
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::library::{BulkEdit, CollectionDetail, SeriesUpdate};
use kitsunarr_domain::api::settings::{GeneralSettings, NamingSettings};
use kitsunarr_domain::api::views::{
    CollectionSettingsUpdate, HostSettings, HostValues, LibraryStatistics, LogFileItem,
    NamingExamples, ParseResult, ParsedEpisode, ParsedRelease, ParsedSeries, StatisticsQuery,
};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

// ---- naming -----------------------------------------------------------------------------------

/// Sonarr's `FileNameSampleService` for the naming settings in the body (unsaved settings
/// preview as they are typed).
#[utoipa::path(post, path = "/settings/naming/examples", tag = "settings", request_body = NamingSettings,
    responses((status = 200, body = NamingExamples)))]
pub async fn naming_examples(
    Json(naming): Json<NamingSettings>,
) -> ApiResult<Json<NamingExamples>> {
    let mut v = crate::v3_extra::naming_examples_for(&naming);
    // Kitsunarr's movie folder (movies are their own library).
    v["movieFolderExample"] = if naming.movie_folder_format.trim().is_empty() {
        serde_json::Value::Null
    } else {
        kitsunarr_domain::naming::folder_name(
            &naming.movie_folder_format,
            "The Movie Title",
            Some(2010),
            naming.colon_replacement_format,
            &naming.custom_colon_replacement_format,
        )
        .into()
    };
    // An empty name is an invalid format.
    if let Some(map) = v.as_object_mut() {
        for value in map.values_mut() {
            if value.as_str().is_some_and(|s| s.trim().is_empty()) {
                *value = serde_json::Value::Null;
            }
        }
    }
    Ok(Json(serde_json::from_value(v).map_err(ApiError::internal)?))
}

// ---- statistics -------------------------------------------------------------------------------

/// Tanukarr's statistics page (Sonarr v5's `GET /statistics`): series by status and type,
/// episodes by state, and series and files by quality profile, file quality and tag.
#[utoipa::path(get, path = "/statistics", tag = "library", params(StatisticsQuery),
    responses((status = 200, body = LibraryStatistics)))]
pub async fn statistics(
    State(state): State<AppState>,
    Query(q): Query<StatisticsQuery>,
) -> ApiResult<Json<LibraryStatistics>> {
    let mut raw = Vec::new();
    if let Some(m) = q.monitored {
        raw.push(format!("monitored={m}"));
    }
    if let Some(t) = q.series_type {
        raw.push(format!("seriesTypes={}", t.as_str()));
    }
    if let Some(p) = q.quality_profile_id {
        raw.push(format!("qualityProfileIds={p}"));
    }
    if let Some(t) = q.tag_id {
        raw.push(format!("tagIds={t}"));
    }
    let v = crate::v5::statistics_json(&state, Some(&raw.join("&"))).await?;
    Ok(Json(serde_json::from_value(v).map_err(ApiError::internal)?))
}

// ---- collections ------------------------------------------------------------------------------

/// Tanukarr's collection settings: the name, kind and film folder style are stored on the
/// collection; monitoring, quality profile and season folders are applied to every member.
#[utoipa::path(put, path = "/collections/{id}/settings", tag = "library", params(("id" = i64, Path)),
    request_body = CollectionSettingsUpdate,
    responses((status = 200, body = CollectionDetail), (status = 400, body = ApiErrorBody), (status = 404, body = ApiErrorBody)))]
pub async fn update_collection_settings(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(u): Json<CollectionSettingsUpdate>,
) -> ApiResult<Json<CollectionDetail>> {
    if let Some(p) = u.quality_profile_id {
        kitsunarr_db::quality::get_profile(state.db.pool(), p)
            .await
            .map_err(|_| {
                ApiError::field("qualityProfileId", "The quality profile does not exist.")
            })?;
    }
    collections::update_settings(
        &state.db,
        id,
        u.title.as_deref(),
        u.group_kind.as_deref(),
        u.movie_folder_style.as_deref(),
    )
    .await?;
    let (series_ids, movie_ids) = collections::member_titles(&state.db, id).await?;
    if (u.monitored.is_some() || u.quality_profile_id.is_some()) && !series_ids.is_empty() {
        let _ = crate::routes::library::bulk_edit_series(
            State(state.clone()),
            Json(BulkEdit {
                ids: series_ids.clone(),
                monitored: u.monitored,
                quality_profile_id: u.quality_profile_id,
                ..Default::default()
            }),
        )
        .await?;
    }
    if (u.monitored.is_some() || u.quality_profile_id.is_some()) && !movie_ids.is_empty() {
        let _ = crate::routes::library::bulk_edit_movies(
            State(state.clone()),
            Json(BulkEdit {
                ids: movie_ids,
                monitored: u.monitored,
                quality_profile_id: u.quality_profile_id,
                ..Default::default()
            }),
        )
        .await?;
    }
    if let Some(season_folder) = u.season_folder {
        for sid in &series_ids {
            series::apply_update(
                &state.db,
                *sid,
                &SeriesUpdate {
                    season_folder: Some(season_folder),
                    ..Default::default()
                },
            )
            .await?;
        }
    }
    state
        .events
        .publish(kitsunarr_domain::api::events::ServerEvent::LibraryChanged {
            media_type: "collection".into(),
            ids: Vec::new(),
            deleted: false,
        });
    Ok(Json(collections::detail(&state.db, id).await?))
}

/// Tanukarr's `SeriesCollectionEditor.Apply` through the series editor: a new name, kind or
/// film folder style for the group of the selected series, which must be every series member
/// of one group.
pub(crate) async fn apply_group_editor(
    state: &AppState,
    series_ids: &[i64],
    title: Option<&str>,
    kind: Option<&str>,
    style: Option<&str>,
) -> ApiResult<()> {
    if title.is_none() && kind.is_none() && style.is_none() {
        return Ok(());
    }
    let mut group = None;
    for id in series_ids {
        let g = series::get_detail(&state.db, *id)
            .await?
            .group_membership
            .map(|g| g.collection_id);
        match (group, g) {
            (_, None) => {
                return Err(ApiError::field(
                    "seriesIds",
                    "Select members of one linked collection",
                ));
            }
            (Some(a), Some(b)) if a != b => {
                return Err(ApiError::field(
                    "seriesIds",
                    "Select members of one linked collection",
                ));
            }
            (_, b) => group = b,
        }
    }
    let Some(group) = group else {
        return Err(ApiError::field(
            "seriesIds",
            "Select the saved collection members",
        ));
    };
    let (mut members, _) = collections::member_titles(&state.db, group).await?;
    let mut selected = series_ids.to_vec();
    members.sort_unstable();
    selected.sort_unstable();
    selected.dedup();
    if members != selected {
        return Err(ApiError::field(
            "seriesIds",
            "Select every member before changing collection settings",
        ));
    }
    let kind = match kind {
        Some(k) => Some(
            kitsunarr_domain::api::views::GROUP_KINDS
                .iter()
                .find(|x| x.eq_ignore_ascii_case(k))
                .copied()
                .ok_or_else(|| ApiError::field("animeGroupKind", "Choose a collection type"))?,
        ),
        None => None,
    };
    let style = match style {
        Some(s) => Some(
            kitsunarr_domain::api::views::MOVIE_FOLDER_STYLES
                .iter()
                .find(|x| x.eq_ignore_ascii_case(s))
                .copied()
                .ok_or_else(|| {
                    ApiError::field("animeGroupMovieFolderStyle", "Choose a movie folder format")
                })?,
        ),
        None => None,
    };
    collections::update_settings(&state.db, group, title, kind, style).await?;
    Ok(())
}

// ---- settings › UI ----------------------------------------------------------------------------

/// Checks Settings › UI values (Sonarr stores any moment format; Kitsunarr keeps them short and
/// the time zone shaped like an IANA name).
pub(crate) fn validate_ui(g: &GeneralSettings) -> ApiResult<()> {
    for (field, value) in [
        ("shortDateFormat", &g.short_date_format),
        ("longDateFormat", &g.long_date_format),
        ("timeFormat", &g.time_format),
        ("calendarWeekColumnHeader", &g.calendar_week_column_header),
    ] {
        if value.trim().is_empty() || value.chars().count() > 64 {
            return Err(ApiError::field(field, "Choose a format."));
        }
    }
    let tz = g.time_zone.trim();
    if !tz.is_empty()
        && (tz.len() > 64
            || !tz
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+')))
    {
        return Err(ApiError::field(
            "timeZone",
            "Use a time zone name such as Europe/Rome, or leave it empty.",
        ));
    }
    Ok(())
}

// ---- settings › general › host ----------------------------------------------------------------

fn running_values(state: &AppState) -> HostValues {
    let c = &state.config;
    HostValues {
        bind_address: c.server.bind_address.to_string(),
        port: c.server.port,
        url_base: c.server.url_base.clone(),
        log_level: c.log.level.clone(),
        backup_folder: c.backup_dir().display().to_string(),
    }
}

/// The overridden keys under the names the API uses.
fn overridden(state: &AppState) -> Vec<String> {
    state
        .config
        .overridden
        .iter()
        .filter_map(|k| {
            Some(match k.as_str() {
                "server.bind_address" => "bindAddress",
                "server.port" => "port",
                "server.url_base" => "urlBase",
                "log.level" => "logLevel",
                "backup_folder" => "backupFolder",
                _ => return None,
            })
        })
        .map(str::to_string)
        .collect()
}

fn host_settings(state: &AppState) -> ApiResult<HostSettings> {
    let file = kitsunarr_config::Config::load_file(&state.config.config_dir)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let running = running_values(state);
    let overridden = overridden(state);
    let url_base = kitsunarr_config::normalize_url_base(&file.server.url_base)
        .unwrap_or_else(|_| file.server.url_base.clone());
    let saved = HostValues {
        bind_address: file.server.bind_address.to_string(),
        port: file.server.port,
        url_base,
        log_level: file.log.level.clone(),
        backup_folder: file
            .backup_folder
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    };
    let is = |k: &str| overridden.iter().any(|o| o == k);
    let default_backups = state.config.data_dir.join("backups").display().to_string();
    let saved_backups = if saved.backup_folder.is_empty() {
        default_backups
    } else {
        saved.backup_folder.clone()
    };
    let restart_required = (!is("bindAddress") && saved.bind_address != running.bind_address)
        || (!is("port") && saved.port != running.port)
        || (!is("urlBase") && saved.url_base != running.url_base)
        || (!is("logLevel") && saved.log_level != running.log_level)
        || (!is("backupFolder") && saved_backups != running.backup_folder);
    Ok(HostSettings {
        bind_address: saved.bind_address,
        port: saved.port,
        url_base: saved.url_base,
        log_level: saved.log_level,
        backup_folder: saved.backup_folder,
        running,
        overridden,
        config_file: state.config.config_file().display().to_string(),
        restart_required,
    })
}

/// Settings › General › Host and Logging: the listening address, port, URL base, log level and
/// backup folder saved in `config.toml`, the values in use, and which are set by environment
/// variables or flags (those win over the file).
#[utoipa::path(get, path = "/settings/host", tag = "settings", responses((status = 200, body = HostSettings)))]
pub async fn get_host(State(state): State<AppState>) -> ApiResult<Json<HostSettings>> {
    Ok(Json(host_settings(&state)?))
}

/// Saves the host values to `config.toml`; they apply when Kitsunarr restarts.
#[utoipa::path(put, path = "/settings/host", tag = "settings", request_body = HostSettings,
    responses((status = 200, body = HostSettings), (status = 400, body = ApiErrorBody)))]
pub async fn put_host(
    State(state): State<AppState>,
    Json(body): Json<HostSettings>,
) -> ApiResult<Json<HostSettings>> {
    let bind_address = body.bind_address.trim().parse().map_err(|_| {
        ApiError::field(
            "bindAddress",
            "Use an IP address (0.0.0.0 for every interface).",
        )
    })?;
    let folder = body.backup_folder.trim();
    let values = kitsunarr_config::HostFileValues {
        bind_address,
        port: body.port,
        url_base: body.url_base.clone(),
        log_level: body.log_level.clone(),
        backup_folder: (!folder.is_empty()).then(|| std::path::PathBuf::from(folder)),
    };
    kitsunarr_config::write_host_values(&state.config.config_dir, &values).map_err(
        |e| match e {
            kitsunarr_config::ConfigError::Invalid { key, message } => {
                let field = match key.as_str() {
                    "server.port" => "port",
                    "server.url_base" => "urlBase",
                    "log.level" => "logLevel",
                    "backup_folder" => "backupFolder",
                    other => other,
                };
                ApiError::field(field, capitalize(&message))
            }
            other => {
                ApiError::BadRequest(format!("The configuration file cannot be saved: {other}"))
            }
        },
    )?;
    tracing::info!(file = %state.config.config_file().display(), "host settings saved; they apply at the next start");
    crate::routes::settings::settings_changed(&state, "host");
    Ok(Json(host_settings(&state)?))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    let mut out = c
        .next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default();
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

// ---- log files --------------------------------------------------------------------------------

/// The rolling log files of `<data dir>/logs`, newest first.
#[utoipa::path(get, path = "/system/logs/files", tag = "system", responses((status = 200, body = Vec<LogFileItem>)))]
pub async fn log_files(State(state): State<AppState>) -> Json<Vec<LogFileItem>> {
    let dir = crate::log_files::folder(&state.config.data_dir);
    Json(
        crate::log_files::list(&dir)
            .into_iter()
            .map(|(name, time)| LogFileItem {
                size: std::fs::metadata(dir.join(&name))
                    .map(|m| m.len())
                    .unwrap_or_default(),
                filename: name,
                last_write_time: chrono::DateTime::<chrono::Utc>::from(time)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            })
            .collect(),
    )
}

#[derive(serde::Deserialize, utoipa::IntoParams, Default)]
#[serde(default)]
pub struct LogFileQuery {
    /// Sends the file as an attachment.
    pub download: bool,
}

/// A log file's text.
#[utoipa::path(get, path = "/system/logs/files/{name}", tag = "system", params(("name" = String, Path), LogFileQuery),
    responses((status = 200, content_type = "text/plain", body = String), (status = 404)))]
pub async fn log_file(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<LogFileQuery>,
) -> ApiResult<Response> {
    if !crate::log_files::valid_name(&name) {
        return Err(ApiError::NotFound("The log file does not exist.".into()));
    }
    let path = crate::log_files::folder(&state.config.data_dir).join(&name);
    let data = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound("The log file does not exist.".into()))?;
    let mut response =
        ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], data).into_response();
    if q.download
        && let Ok(v) = format!("attachment; filename=\"{name}\"").parse()
    {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

// ---- parse ------------------------------------------------------------------------------------

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct ParseQuery {
    /// A release title or file name.
    pub title: String,
}

/// Tanukarr's "Test parsing": what the parser reads from a release title, and the library
/// series and episodes it maps to.
#[utoipa::path(get, path = "/parse", tag = "library", params(ParseQuery),
    responses((status = 200, body = ParseResult), (status = 400, body = ApiErrorBody)))]
pub async fn parse(
    State(state): State<AppState>,
    Query(q): Query<ParseQuery>,
) -> ApiResult<Json<ParseResult>> {
    let title = q.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::field("title", "Give a title to parse."));
    }
    let Some(p) = kitsunarr_parser::parse_title(&title) else {
        return Ok(Json(ParseResult {
            title,
            ..Default::default()
        }));
    };
    let mut out = ParseResult {
        title: title.clone(),
        parsed: Some(ParsedRelease {
            release_title: p.release_title.clone(),
            series_title: p.series_title.clone(),
            season_numbers: p.season_numbers.clone(),
            episode_numbers: p.episode_numbers.clone(),
            absolute_episode_numbers: p.absolute_episode_numbers.clone(),
            air_date: p.air_date.clone(),
            quality: p.quality.quality.name.to_string(),
            quality_source: serde_json::to_value(p.quality.quality.source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            resolution: Some(p.quality.quality.resolution).filter(|r| *r > 0),
            revision: p.quality.revision.version,
            is_repack: p.quality.revision.is_repack,
            languages: p.languages.iter().map(|l| l.name.to_string()).collect(),
            release_group: p.release_group.clone(),
            release_hash: Some(p.release_hash.clone()).filter(|h| !h.is_empty()),
            full_season: p.full_season,
            is_partial_season: p.is_partial_season,
            is_multi_season: p.is_multi_season(),
            is_season_extra: p.is_season_extra,
            special: p.special,
            is_daily: p.is_daily(),
            is_absolute_numbering: p.is_absolute_numbering(),
            is_possible_special_episode: p.is_possible_special_episode(),
        }),
        ..Default::default()
    };
    let series_id = kitsunarr_library::manual_import::series_named(&state.db, &title)
        .await
        .map_err(ApiError::internal)?;
    if let Some(id) = series_id {
        let detail = series::get_detail(&state.db, id).await?;
        let episodes = series::list_episodes(&state.db, id, None).await?;
        let model =
            kitsunarr_library::decision_input::series_model(&state.db, &detail, &episodes).await?;
        let mapped: Vec<i64> = kitsunarr_decision::map_episodes(&p, &model)
            .iter()
            .map(|e| e.id)
            .collect();
        out.series = Some(ParsedSeries {
            id,
            title: detail.item.title.clone(),
            year: detail.item.year,
        });
        out.episodes = episodes
            .iter()
            .filter(|e| mapped.contains(&e.id))
            .map(|e| ParsedEpisode {
                id: e.id,
                season_number: e.season_number,
                episode_number: e.episode_number,
                absolute_episode_number: e.absolute_episode_number,
                title: e.title.clone(),
                air_date: e.air_date.clone(),
                has_file: e.file.is_some(),
            })
            .collect();
    }
    Ok(Json(out))
}
