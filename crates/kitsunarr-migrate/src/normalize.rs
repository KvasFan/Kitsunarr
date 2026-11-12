//! Turns raw records (SQLite rows or API resources) into a [`SourceSnapshot`].
//!
//! The same functions handle both encodings: integers or strings for enums, JSON-as-text or
//! inline JSON, `{quality: 7}` or `{quality: {id: 7}}`, `[{type, body}]` or `[{fields}]`
//! custom format specifications, provider `Settings` objects or API `fields` arrays.

use std::collections::HashMap;

use kitsunarr_domain::api::settings::{CustomFormatSpecification, ProviderKind};
use kitsunarr_domain::{MediaImage, MetadataProvider, MonitorNewItems, SeriesStatus, SeriesType};
use serde_json::{Map, Value, json};

use crate::model::*;
use crate::values::{Rec, as_bool, as_i64};

const MASK: &str = "********";

/// Source config keys that are implementation internals, not user settings.
const INTERNAL_CONFIG_KEYS: &[&str] = &[
    "rijndaelpassphrase",
    "rijndaelsalt",
    "hmacpassphrase",
    "hmacsalt",
    "plexclientidentifier",
    "cleanupmetadataimages",
    "migratenamingconfig",
    "downloadclientworkingfolders",
];

pub fn normalize(raw: &RawSource) -> SourceSnapshot {
    let mut snap = SourceSnapshot {
        tags: raw
            .table("tags")
            .iter()
            .filter_map(|r| Some((r.i64("id")?, r.str("label")?)))
            .collect(),
        root_folders: raw
            .table("rootfolders")
            .iter()
            .filter_map(|r| Some((r.i64("id")?, r.str("path")?)))
            .collect(),
        quality_definitions: raw
            .table("qualitydefinitions")
            .iter()
            .filter_map(quality_definition)
            .collect(),
        custom_formats: raw
            .table("customformats")
            .iter()
            .filter_map(custom_format)
            .collect(),
        quality_profiles: raw
            .table("qualityprofiles")
            .iter()
            .filter_map(quality_profile)
            .collect(),
        delay_profiles: raw
            .table("delayprofiles")
            .iter()
            .filter_map(delay_profile)
            .collect(),
        release_profiles: raw
            .table("releaseprofiles")
            .iter()
            .filter_map(release_profile)
            .collect(),
        exclusions: raw
            .table("importlistexclusions")
            .iter()
            .filter_map(exclusion)
            .collect(),
        remote_path_mappings: raw
            .table("remotepathmappings")
            .iter()
            .filter_map(|r| {
                Some((
                    r.i64("id")?,
                    r.str("host")?,
                    r.str("remotepath")?,
                    r.str("localpath")?,
                ))
            })
            .collect(),
        naming: raw.table("namingconfig").first().cloned(),
        ..Default::default()
    };
    for (table, kind) in [
        ("indexers", ProviderKind::Indexer),
        ("downloadclients", ProviderKind::DownloadClient),
        ("notifications", ProviderKind::Notification),
        ("importlists", ProviderKind::ImportList),
    ] {
        snap.providers
            .extend(raw.table(table).iter().filter_map(|r| provider(r, kind)));
    }

    // Media management: API object, or the Config key/value table of a database.
    if let Some(mm) = raw.table("mediamanagement").first() {
        snap.media_management = Some(mm.clone());
    }
    if let Some(io) = raw.table("indexerconfig").first() {
        snap.indexer_options = Some(io.clone());
    }
    if let Some(h) = raw.table("hostconfig").first() {
        snap.host_options = Some(h.clone());
    }
    if let Some(dc) = raw.table("downloadclientconfig").first() {
        snap.download_client_options = Some(dc.clone());
    }
    if let Some(il) = raw.table("importlistconfig").first() {
        snap.import_list_options = Some(il.clone());
    }
    if let Some(cfg) = raw.table("config").first() {
        let mut mm = Map::new();
        for (k, v) in &cfg.0 {
            if INTERNAL_CONFIG_KEYS.contains(&k.as_str()) {
                snap.internal_settings_skipped += 1;
                continue;
            }
            mm.insert(k.clone(), v.clone());
        }
        snap.media_management = Some(Rec(mm));
    }
    if let Some(mm) = &snap.media_management {
        for (k, v) in &mm.0 {
            if !crate::import::MAPPED_MEDIA_MANAGEMENT_KEYS.contains(&k.as_str()) && k != "id" {
                snap.unmapped_settings.insert(k.clone(), v.clone());
            }
        }
    }

    // Scene-mapping titles by TVDB id (database backups only).
    let mut scene: HashMap<i64, Vec<(String, Option<i32>)>> = HashMap::new();
    for r in raw.table("scenemappings") {
        if let (Some(tvdb), Some(title)) = (r.i64("tvdbid"), r.str("title")) {
            let season = r.i32("seasonnumber").filter(|s| *s >= 0);
            scene.entry(tvdb).or_default().push((title, season));
        }
    }
    snap.series = raw
        .table("series")
        .iter()
        .filter_map(|r| series(r, &scene))
        .collect();
    snap.episodes = raw.table("episodes").iter().filter_map(episode).collect();
    snap.episode_files = raw
        .table("episodefiles")
        .iter()
        .filter_map(episode_file)
        .collect();
    snap.history = raw.table("history").iter().filter_map(history).collect();
    snap.retained_versions = raw
        .table("retainedversions")
        .iter()
        .filter_map(retained_version)
        .collect();
    snap
}

/// A `RetainedVersions` row; its `MetadataJson` is the replaced `EpisodeFile` (Newtonsoft,
/// camelCase).
fn retained_version(r: &Rec) -> Option<SrcRetainedVersion> {
    let series_id = r.i64("seriesid")?;
    let mut metadata = match r.json("metadatajson")? {
        Value::Object(m) => Rec::from_map(m),
        _ => return None,
    };
    metadata.0.insert("id".into(), json!(0));
    metadata.0.insert("seriesid".into(), json!(series_id));
    let episode_ids = match r.json("episodeidsjson") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_i64).collect(),
        _ => Vec::new(),
    };
    Some(SrcRetainedVersion {
        id: r.i64("id")?,
        series_id,
        archive_path: r.str("relativepath")?.replace('\\', "/"),
        file: episode_file(&metadata)?,
        episode_ids,
        created: r.ts("created"),
    })
}

// ---------------------------------------------------------------------------------------------
// Enum decoding (integer in SQLite, camelCase string in the API)
// ---------------------------------------------------------------------------------------------

fn enum_code(v: Option<&Value>) -> Option<Result<i64, String>> {
    let v = v?;
    Some(match v {
        Value::String(s) => match s.trim().parse::<i64>() {
            Ok(i) => Ok(i),
            Err(_) => Err(s.trim().to_ascii_lowercase()),
        },
        other => Ok(as_i64(other)?),
    })
}

pub fn series_status(v: Option<&Value>) -> SeriesStatus {
    match enum_code(v) {
        Some(Ok(0)) => SeriesStatus::Continuing,
        Some(Ok(1)) => SeriesStatus::Ended,
        Some(Ok(2)) => SeriesStatus::Upcoming,
        Some(Ok(-1)) => SeriesStatus::Deleted,
        Some(Err(s)) => s.parse().unwrap_or(SeriesStatus::Unknown),
        _ => SeriesStatus::Unknown,
    }
}

pub fn series_type(v: Option<&Value>) -> SeriesType {
    match enum_code(v) {
        Some(Ok(1)) => SeriesType::Daily,
        Some(Ok(2)) => SeriesType::Anime,
        Some(Err(s)) => s.parse().unwrap_or(SeriesType::Standard),
        _ => SeriesType::Standard,
    }
}

pub fn monitor_new_items(v: Option<&Value>) -> MonitorNewItems {
    match enum_code(v) {
        Some(Ok(1)) => MonitorNewItems::None,
        Some(Err(s)) if s == "none" => MonitorNewItems::None,
        _ => MonitorNewItems::All,
    }
}

/// Tanukarr `MetadataSourceType`: Tvdb=0, Tmdb=1, AniList=2, MyAnimeList=3, TvMaze=4, TmdbMovie=5, Omdb=6.
pub fn metadata_source(v: Option<&Value>) -> Option<MetadataProvider> {
    match enum_code(v)? {
        Ok(0) => Some(MetadataProvider::Tvdb),
        Ok(1) => Some(MetadataProvider::TmdbTv),
        Ok(2) => Some(MetadataProvider::AniList),
        Ok(3) => Some(MetadataProvider::MyAnimeList),
        Ok(4) => Some(MetadataProvider::TvMaze),
        Ok(5) => Some(MetadataProvider::TmdbMovie),
        Ok(6) => Some(MetadataProvider::Omdb),
        Ok(_) => None,
        Err(s) => MetadataProvider::ALL
            .iter()
            .copied()
            .find(|p| p.as_str().eq_ignore_ascii_case(&s)),
    }
}

/// Tanukarr `MediaType`: Series=0, Movie=1, Special=2.
pub fn media_type(v: Option<&Value>) -> SrcMediaType {
    match enum_code(v) {
        Some(Ok(1)) => SrcMediaType::Movie,
        Some(Ok(2)) => SrcMediaType::Special,
        Some(Err(s)) if s == "movie" => SrcMediaType::Movie,
        Some(Err(s)) if s == "special" => SrcMediaType::Special,
        _ => SrcMediaType::Series,
    }
}

const HISTORY_EVENTS: &[&str] = &[
    "unknown",
    "grabbed",
    "seriesFolderImported",
    "downloadFolderImported",
    "downloadFailed",
    "episodeFileDeleted",
    "episodeFileRenamed",
    "downloadIgnored",
];

fn history_event(v: Option<&Value>) -> String {
    match enum_code(v) {
        Some(Ok(i)) => HISTORY_EVENTS
            .get(usize::try_from(i).unwrap_or(0))
            .copied()
            .unwrap_or("unknown")
            .to_string(),
        Some(Err(s)) => HISTORY_EVENTS
            .iter()
            .find(|e| e.eq_ignore_ascii_case(&s))
            .map_or(s, |e| (*e).to_string()),
        None => "unknown".into(),
    }
}

// ---------------------------------------------------------------------------------------------
// Shared structures
// ---------------------------------------------------------------------------------------------

fn images(values: &[Value]) -> Vec<MediaImage> {
    values
        .iter()
        .filter_map(|v| {
            let r = Rec::from_value(v.clone());
            let url = r.str("remoteurl").or_else(|| r.str("url"))?;
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            let cover = match r.raw("covertype") {
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(1) => "poster",
                    Some(2) => "banner",
                    Some(3) => "fanart",
                    Some(4) => "screenshot",
                    Some(5) => "headshot",
                    Some(6) => "clearlogo",
                    _ => "unknown",
                }
                .to_string(),
                _ => r
                    .str("covertype")
                    .unwrap_or_else(|| "unknown".into())
                    .to_ascii_lowercase(),
            };
            Some(MediaImage {
                cover_type: cover,
                remote_url: url,
            })
        })
        .collect()
}

/// `{"quality": 7, "revision": {...}}` or `{"quality": {"id": 7, ...}, "revision": {...}}`.
pub fn quality_model(v: Option<Value>) -> Option<(i32, SrcRevision)> {
    let r = Rec::from_value(v?);
    let id = match r.raw("quality")? {
        Value::Object(o) => o.get("id").and_then(as_i64),
        other => as_i64(other),
    }?;
    let rev = r
        .object("revision")
        .map_or_else(SrcRevision::default, |rev| SrcRevision {
            version: rev.i32("version").unwrap_or(1),
            real: rev.i32("real").unwrap_or(0),
            is_repack: rev.bool_or("isrepack", false),
        });
    Some((i32::try_from(id).ok()?, rev))
}

fn language_ids(r: &Rec, key: &str) -> Vec<i32> {
    r.int_list(key)
        .into_iter()
        .filter_map(|i| i32::try_from(i).ok())
        .collect()
}

fn note(
    unmapped: &mut Map<String, Value>,
    r: &Rec,
    key: &str,
    is_default: impl Fn(&Value) -> bool,
) {
    if let Some(v) = r.json(key).filter(|v| !v.is_null() && !is_default(v)) {
        unmapped.insert(key.to_string(), v);
    }
}

fn zero_or_empty(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty() || s == "0",
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Bool(b) => !b,
        Value::Null => true,
    }
}

// ---------------------------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------------------------

fn parent_dir(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let idx = trimmed.rfind(['/', '\\'])?;
    let parent = &trimmed[..idx];
    Some(if parent.is_empty() {
        trimmed[..=idx].to_string()
    } else {
        parent.to_string()
    })
}

fn series(r: &Rec, scene: &HashMap<i64, Vec<(String, Option<i32>)>>) -> Option<SrcSeries> {
    let id = r.i64("id")?;
    let title = r.str("title")?;
    let path = r.str("path")?;
    let tvdb_id = r.i64("tvdbid").unwrap_or(0);
    let mut unmapped = Map::new();
    for key in [
        "orderingid",
        "pendingpath",
        "language",
        "animegroupfolder",
        "animegroupmemberfolder",
        "animegroupmoviefolderstyle",
    ] {
        note(&mut unmapped, r, key, zero_or_empty);
    }
    note(&mut unmapped, r, "seasontype", |v| {
        v.as_str()
            .is_none_or(|s| s.eq_ignore_ascii_case("official"))
    });

    let mut alternate_titles: Vec<(String, Option<i32>)> = r
        .array("alternatetitles")
        .into_iter()
        .filter_map(|v| match v {
            Value::String(s) => Some((s, None)),
            Value::Object(o) => {
                let a = Rec::from_map(o);
                Some((
                    a.str("title")?,
                    a.i32("seasonnumber")
                        .or_else(|| a.i32("sceneseasonnumber"))
                        .filter(|s| *s >= 0),
                ))
            }
            _ => None,
        })
        .collect();
    if tvdb_id > 0
        && let Some(extra) = scene.get(&tvdb_id)
    {
        alternate_titles.extend(extra.iter().cloned());
    }
    alternate_titles.retain(|(t, _)| !t.eq_ignore_ascii_case(&title));
    alternate_titles.sort();
    alternate_titles.dedup();

    let group = r.str("animegroupid").map(|group_id| SrcGroup {
        group_id: Some(group_id),
        group_title: r.str("animegrouptitle"),
        group_kind: kind_code(r.raw("animegroupkind"), &["series", "moviesaga"]),
        grouping_mode: kind_code(
            r.raw("animegroupingmode"),
            &["automatic", "manual", "disabled"],
        ),
        member_kind: kind_code(
            r.raw("animegroupmemberkind"),
            &["season", "movie", "special"],
        ),
        season_number: r.i64("animegroupseasonnumber"),
        movie_number: r.i64("animegroupmovienumber"),
        special_number: r.i64("animegroupspecialnumber"),
    });

    Some(SrcSeries {
        id,
        status: series_status(r.raw("status")),
        overview: r.str("overview"),
        network: r.str("network"),
        air_time: r.str("airtime"),
        images: images(&r.array("images")),
        root_folder_path: r.str("rootfolderpath").or_else(|| parent_dir(&path)),
        path,
        monitored: r.bool_or("monitored", true),
        monitor_new_items: monitor_new_items(r.raw("monitornewitems")),
        season_folder: r.bool_or("seasonfolder", true),
        use_scene_numbering: r.bool_or("usescenenumbering", false),
        series_type: series_type(r.raw("seriestype")),
        runtime: r.i32("runtime").filter(|v| *v > 0),
        year: r.i32("year").filter(|v| *v > 0),
        first_aired: r.date("firstaired"),
        last_aired: r.date("lastaired"),
        certification: r.str("certification"),
        genres: r.string_list("genres"),
        ratings: r.json("ratings").filter(|v| v.is_object()),
        original_language: match r.raw("originallanguage") {
            Some(Value::Object(o)) => o
                .get("id")
                .and_then(as_i64)
                .and_then(|i| i32::try_from(i).ok()),
            _ => r.i32("originallanguage"),
        },
        quality_profile_id: r
            .i64("qualityprofileid")
            .or_else(|| r.i64("profileid"))
            .unwrap_or(0),
        tags: r.int_list("tags"),
        added: r.ts("added"),
        last_info_sync: r.ts("lastinfosync"),
        seasons: r
            .array("seasons")
            .into_iter()
            .filter_map(|v| {
                let s = Rec::from_value(v);
                Some(SrcSeason {
                    number: s.i32("seasonnumber")?,
                    monitored: s.bool_or("monitored", true),
                    images: images(&s.array("images")),
                })
            })
            .collect(),
        tvdb_id,
        tv_rage_id: r.i64("tvrageid").unwrap_or(0),
        tv_maze_id: r.i64("tvmazeid").unwrap_or(0),
        tmdb_id: r.i64("tmdbid").unwrap_or(0),
        imdb_id: r.str("imdbid"),
        mal_ids: r.int_list("malids"),
        anilist_ids: r.int_list("anilistids"),
        alternate_titles,
        metadata_source: metadata_source(r.raw("metadatasource")),
        foreign_id: r.str("foreignid"),
        media_type: media_type(r.raw("mediatype")),
        ordering_id: r.str("orderingid"),
        collection_title: r.str("collectiontitle"),
        group,
        original_title: r.str("originaltitle"),
        title,
        unmapped,
    })
}

fn kind_code(v: Option<&Value>, names: &[&str]) -> i64 {
    match enum_code(v) {
        Some(Ok(i)) => i,
        Some(Err(s)) => names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&s))
            .map_or(0, |p| p as i64),
        None => 0,
    }
}

fn episode(r: &Rec) -> Option<SrcEpisode> {
    let mut unmapped = Map::new();
    note(&mut unmapped, r, "watched", zero_or_empty);
    note(&mut unmapped, r, "watchedatutc", zero_or_empty);
    Some(SrcEpisode {
        id: r.i64("id")?,
        series_id: r.i64("seriesid")?,
        season_number: r.i32("seasonnumber")?,
        episode_number: r.i32("episodenumber")?,
        absolute_episode_number: r.i32("absoluteepisodenumber").filter(|v| *v > 0),
        scene_absolute_episode_number: r.i32("sceneabsoluteepisodenumber").filter(|v| *v > 0),
        scene_season_number: r.i32("sceneseasonnumber"),
        scene_episode_number: r.i32("sceneepisodenumber"),
        unverified_scene_numbering: r.bool_or("unverifiedscenenumbering", false),
        title: r.str("title"),
        overview: r.str("overview"),
        air_date: r.date("airdate"),
        air_date_utc: r.ts("airdateutc"),
        runtime: r.i32("runtime").filter(|v| *v > 0),
        monitored: r.bool_or("monitored", true),
        episode_file_id: r.i64("episodefileid").filter(|v| *v > 0),
        last_search_time: r.ts("lastsearchtime"),
        provider_episode_id: r
            .str("foreignid")
            .or_else(|| r.i64("tvdbid").filter(|v| *v > 0).map(|v| v.to_string())),
        unmapped,
    })
}

fn episode_file(r: &Rec) -> Option<SrcEpisodeFile> {
    let (quality_id, revision) =
        quality_model(r.json("quality")).unwrap_or((0, SrcRevision::default()));
    let mut unmapped = Map::new();
    note(&mut unmapped, r, "sourcereleasetitle", zero_or_empty);
    note(&mut unmapped, r, "sourcereleasesize", zero_or_empty);
    let relative_path = r.str("relativepath").or_else(|| {
        // API resources always have relativePath; very old databases may only have Path.
        r.str("path")
            .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string())
    })?;
    Some(SrcEpisodeFile {
        id: r.i64("id")?,
        series_id: r.i64("seriesid")?,
        season_number: r.i32("seasonnumber").unwrap_or(1),
        relative_path,
        size: r.i64("size").unwrap_or(0),
        date_added: r.ts("dateadded"),
        scene_name: r.str("scenename"),
        release_group: r.str("releasegroup"),
        quality_id,
        revision,
        languages: language_ids(r, "languages"),
        media_info: r.json("mediainfo").filter(|v| v.is_object()),
        original_file_path: r.str("originalfilepath"),
        indexer_flags: r.i64("indexerflags").unwrap_or(0),
        release_type: match r.raw("releasetype") {
            Some(Value::String(s)) if s.parse::<i64>().is_err() => [
                "unknown",
                "singleEpisode",
                "multiEpisode",
                "seasonPack",
                "multiSeasonPack",
            ]
            .iter()
            .position(|n| n.eq_ignore_ascii_case(s))
            .unwrap_or(0) as i64,
            _ => r.i64("releasetype").unwrap_or(0),
        },
        unmapped,
    })
}

fn profile_item(v: &Value, sizes: &mut Vec<SrcSizeLimit>) -> Option<SrcProfileItem> {
    let r = Rec::from_value(v.clone());
    let allowed = r.bool_or("allowed", false);
    if let Some(q) = r.raw("quality") {
        let id = match q {
            Value::Object(o) => o.get("id").and_then(as_i64),
            other => as_i64(other),
        }?;
        // Sonarr v5 and Tanukarr keep size limits on profile items (MiB per minute).
        if (r.has("minsize") || r.has("maxsize") || r.has("preferredsize"))
            && let Ok(quality_id) = i32::try_from(id)
        {
            sizes.push(SrcSizeLimit {
                quality_id,
                min_size: r.f64("minsize"),
                preferred_size: r.f64("preferredsize"),
                max_size: r.f64("maxsize").filter(|m| *m > 0.0),
            });
        }
        return Some(SrcProfileItem {
            quality_id: i32::try_from(id).ok(),
            group: None,
            allowed,
        });
    }
    let gid = r.i32("id")?;
    let name = r.str("name").unwrap_or_else(|| format!("Group {gid}"));
    let members: Vec<i32> = r
        .array("items")
        .iter()
        .filter_map(|sub| profile_item(sub, sizes).and_then(|i| i.quality_id))
        .collect();
    Some(SrcProfileItem {
        quality_id: None,
        group: Some((gid, name, members)),
        allowed,
    })
}

fn quality_profile(r: &Rec) -> Option<SrcQualityProfile> {
    let mut size_limits = Vec::new();
    let items: Vec<SrcProfileItem> = r
        .array("items")
        .iter()
        .filter_map(|v| profile_item(v, &mut size_limits))
        .collect();
    let mut unmapped = Map::new();
    // Tanukarr's codec factors: a JSON object (a string in databases).
    let codec_multipliers: Vec<(String, f64)> = match r.json("codecmultipliers") {
        Some(Value::Object(m)) => m
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), as_f64(v)?)))
            .collect(),
        _ => Vec::new(),
    };
    // Sonarr v3 language profiles were folded into custom formats in v4; keep the reference.
    note(&mut unmapped, r, "language", zero_or_empty);
    Some(SrcQualityProfile {
        id: r.i64("id")?,
        name: r.str("name")?,
        upgrade_allowed: r.bool_or("upgradeallowed", true),
        cutoff: r.i32("cutoff").unwrap_or(0),
        items,
        format_items: r
            .array("formatitems")
            .into_iter()
            .filter_map(|v| {
                let f = Rec::from_value(v);
                Some((f.i64("format")?, f.i32("score").unwrap_or(0)))
            })
            .collect(),
        min_format_score: r.i32("minformatscore").unwrap_or(0),
        cutoff_format_score: r.i32("cutoffformatscore").unwrap_or(0),
        min_upgrade_format_score: r.i32("minupgradeformatscore").unwrap_or(1),
        size_limits,
        codec_multipliers,
        unmapped,
    })
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())
}

fn quality_definition(r: &Rec) -> Option<SrcQualityDefinition> {
    let id = match r.raw("quality")? {
        Value::Object(o) => o.get("id").and_then(as_i64),
        other => as_i64(other),
    }?;
    Some(SrcQualityDefinition {
        quality_id: i32::try_from(id).ok()?,
        title: r.str("title").unwrap_or_default(),
        min_size: r.f64("minsize"),
        preferred_size: r.f64("preferredsize"),
        max_size: r.f64("maxsize"),
        // A number in Tanukarr's database, a camelCase name in its API.
        source: match r.raw("source") {
            Some(Value::String(s)) => s
                .parse::<i64>()
                .ok()
                .and_then(kitsunarr_domain::quality::QualitySource::from_sonarr_id)
                .or_else(|| serde_json::from_value(Value::String(s.clone())).ok()),
            Some(v) => as_i64(v).and_then(kitsunarr_domain::quality::QualitySource::from_sonarr_id),
            None => None,
        },
        resolution: r.i32("resolution"),
        codec: r.str("codec").unwrap_or_default(),
        title_pattern: r.str("titlepattern").unwrap_or_default(),
        is_retired: r.bool("isretired").unwrap_or(false),
    })
}

const SPEC_META_KEYS: &[&str] = &[
    "order",
    "implementationname",
    "infolink",
    "name",
    "negate",
    "required",
    "presets",
];

fn custom_format(r: &Rec) -> Option<SrcCustomFormat> {
    let specifications = r
        .array("specifications")
        .into_iter()
        .filter_map(|v| {
            let s = Rec::from_value(v);
            // Database: {"type": "...", "body": {...}}; API: {"implementation": "...", "fields": [...]}
            if let Some(body) = s.object("body") {
                let fields: Map<String, Value> = body
                    .0
                    .iter()
                    .filter(|(k, _)| !SPEC_META_KEYS.contains(&k.as_str()))
                    .map(|(k, v)| (camel(k, &body_original_keys(&s)), v.clone()))
                    .collect();
                return Some(CustomFormatSpecification {
                    name: body.str("name").unwrap_or_default(),
                    implementation: s.str("type")?,
                    negate: body.bool_or("negate", false),
                    required: body.bool_or("required", false),
                    fields,
                });
            }
            let fields: Map<String, Value> = s
                .array("fields")
                .into_iter()
                .filter_map(|f| {
                    let f = Rec::from_value(f);
                    Some((
                        f.str("name")?,
                        f.raw("value").cloned().unwrap_or(Value::Null),
                    ))
                })
                .collect();
            Some(CustomFormatSpecification {
                name: s.str("name").unwrap_or_default(),
                implementation: s.str("implementation")?,
                negate: s.bool_or("negate", false),
                required: s.bool_or("required", false),
                fields,
            })
        })
        .collect();
    Some(SrcCustomFormat {
        id: r.i64("id")?,
        name: r.str("name")?,
        include_when_renaming: r.bool_or("includecustomformatwhenrenaming", false),
        specifications,
    })
}

/// Original (camelCase) keys of a `{type, body}` specification body.
fn body_original_keys(spec: &Rec) -> Vec<String> {
    match spec.raw("body") {
        Some(Value::Object(o)) => o.keys().cloned().collect(),
        Some(Value::String(s)) => serde_json::from_str::<Map<String, Value>>(s)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn camel(lower: &str, originals: &[String]) -> String {
    originals
        .iter()
        .find(|o| o.eq_ignore_ascii_case(lower))
        .cloned()
        .unwrap_or_else(|| lower.to_string())
}

fn provider(r: &Rec, kind: ProviderKind) -> Option<SrcProvider> {
    let mut settings = Map::new();
    let mut masked = Vec::new();
    match r.raw("fields") {
        // API resource: fields array; secrets are masked.
        Some(Value::Array(fields)) => {
            for f in fields {
                let f = Rec::from_value(f.clone());
                let Some(name) = f.str("name") else { continue };
                let value = f.raw("value").cloned().unwrap_or(Value::Null);
                if value.as_str() == Some(MASK) {
                    masked.push(name.clone());
                    settings.insert(name, Value::String(String::new()));
                } else {
                    settings.insert(name, value);
                }
            }
        }
        // Database row: Settings JSON with the real values.
        _ => {
            if let Some(Value::Object(m)) = r.json("settings") {
                settings = m;
            }
        }
    }
    let mut options = Map::new();
    let enabled = match kind {
        ProviderKind::Indexer => {
            for key in [
                "enablerss",
                "enableautomaticsearch",
                "enableinteractivesearch",
            ] {
                options.insert(camel_key(key), Value::Bool(r.bool_or(key, true)));
            }
            if let Some(id) = r.i64("downloadclientid").filter(|v| *v > 0) {
                options.insert("downloadClientId".into(), id.into());
            }
            options.insert(
                "seasonSearchMaximumSingleEpisodeAge".into(),
                r.i64("seasonsearchmaximumsingleepisodeage")
                    .unwrap_or(0)
                    .into(),
            );
            [
                "enablerss",
                "enableautomaticsearch",
                "enableinteractivesearch",
            ]
            .iter()
            .any(|k| r.bool_or(k, true))
        }
        ProviderKind::DownloadClient => {
            options.insert(
                "removeCompletedDownloads".into(),
                r.bool_or("removecompleteddownloads", true).into(),
            );
            options.insert(
                "removeFailedDownloads".into(),
                r.bool_or("removefaileddownloads", true).into(),
            );
            r.bool_or("enable", true)
        }
        ProviderKind::Notification => {
            for (k, v) in &r.0 {
                if ((k.starts_with("on") && k.len() > 2) || k == "includehealthwarnings")
                    && let Some(b) = as_bool(v)
                {
                    options.insert(camel_key(k), Value::Bool(b));
                }
            }
            options.values().any(|v| v.as_bool() == Some(true))
        }
        ProviderKind::ImportList => {
            for key in [
                "enableautomaticadd",
                "rootfolderpath",
                "shouldmonitor",
                "qualityprofileid",
                "seriestype",
                "seasonfolder",
                "searchformissingepisodes",
                "monitornewitems",
                "preferredmetadatasource",
                "tagexisting",
                "language",
                "listtype",
                "minrefreshinterval",
            ] {
                if let Some(v) = r.raw(key) {
                    options.insert(camel_key(key), v.clone());
                }
            }
            r.bool_or("enableautomaticadd", true)
        }
    };
    Some(SrcProvider {
        kind,
        id: r.i64("id")?,
        name: r.str("name")?,
        implementation: r.str("implementation")?,
        config_contract: r.str("configcontract"),
        enabled,
        priority: r
            .i32("priority")
            .unwrap_or(if kind == ProviderKind::Indexer { 25 } else { 1 }),
        tags: r.int_list("tags"),
        options,
        settings,
        masked_fields: masked,
    })
}

/// Lower-case source key → Kitsunarr camelCase option name.
fn camel_key(lower: &str) -> String {
    const KNOWN: &[&str] = &[
        "enableRss",
        "enableAutomaticSearch",
        "enableInteractiveSearch",
        "onGrab",
        "onDownload",
        "onUpgrade",
        "onRename",
        "onSeriesAdd",
        "onSeriesDelete",
        "onEpisodeFileDelete",
        "onEpisodeFileDeleteForUpgrade",
        "onHealthIssue",
        "onHealthRestored",
        "onApplicationUpdate",
        "onManualInteractionRequired",
        "onImportComplete",
        "includeHealthWarnings",
        "enableAutomaticAdd",
        "rootFolderPath",
        "shouldMonitor",
        "qualityProfileId",
        "seriesType",
        "seasonFolder",
        "searchForMissingEpisodes",
        "monitorNewItems",
        "preferredMetadataSource",
        "tagExisting",
        "language",
        "listType",
        "minRefreshInterval",
    ];
    KNOWN
        .iter()
        .find(|k| k.eq_ignore_ascii_case(lower))
        .map_or_else(|| lower.to_string(), |k| (*k).to_string())
}

fn delay_profile(r: &Rec) -> Option<SrcDelayProfile> {
    let protocol = match enum_code(r.raw("preferredprotocol")) {
        Some(Ok(2)) => "torrent",
        Some(Err(s)) if s == "torrent" => "torrent",
        _ => "usenet",
    };
    Some(SrcDelayProfile {
        id: r.i64("id")?,
        enable_usenet: r.bool_or("enableusenet", true),
        enable_torrent: r.bool_or("enabletorrent", true),
        preferred_protocol: protocol.into(),
        usenet_delay: r.i64("usenetdelay").unwrap_or(0),
        torrent_delay: r.i64("torrentdelay").unwrap_or(0),
        bypass_if_highest_quality: r.bool_or("bypassifhighestquality", false),
        bypass_if_above_custom_format_score: r.bool_or("bypassifabovecustomformatscore", false),
        minimum_custom_format_score: r.i64("minimumcustomformatscore").unwrap_or(0),
        order: r.i64("order").unwrap_or(i64::from(i32::MAX)),
        tags: r.int_list("tags"),
    })
}

fn release_profile(r: &Rec) -> Option<SrcReleaseProfile> {
    let mut indexer_ids = r.int_list("indexerids");
    if indexer_ids.is_empty()
        && let Some(id) = r.i64("indexerid").filter(|v| *v > 0)
    {
        indexer_ids.push(id);
    }
    indexer_ids.retain(|v| *v > 0);
    let mut unmapped = Map::new();
    for key in [
        "excludedtags",
        "airdaterestriction",
        "airdategraceperiod",
        "allowseasonpackwithoutallepisodesaired",
        "preferred",
    ] {
        note(&mut unmapped, r, key, zero_or_empty);
    }
    Some(SrcReleaseProfile {
        id: r.i64("id")?,
        name: r.str("name"),
        enabled: r.bool_or("enabled", true),
        required: r.string_list("required"),
        ignored: r.string_list("ignored"),
        indexer_ids,
        tags: r.int_list("tags"),
        unmapped,
    })
}

fn exclusion(r: &Rec) -> Option<SrcExclusion> {
    let source = metadata_source(r.raw("metadatasource")).unwrap_or(MetadataProvider::Tvdb);
    let external_id = match source {
        MetadataProvider::Tvdb => r
            .i64("tvdbid")
            .filter(|v| *v > 0)
            .map(|v| v.to_string())
            .or_else(|| r.str("foreignid")),
        _ => r
            .str("foreignid")
            .or_else(|| r.i64("tvdbid").filter(|v| *v > 0).map(|v| v.to_string())),
    }?;
    Some(SrcExclusion {
        id: r.i64("id")?,
        provider: source,
        external_id,
        title: r.str("title"),
    })
}

fn history(r: &Rec) -> Option<SrcHistory> {
    let (quality_id, revision) = match quality_model(r.json("quality")) {
        Some((q, rev)) => (Some(q), Some(rev)),
        None => (None, None),
    };
    Some(SrcHistory {
        id: r.i64("id")?,
        series_id: r.i64("seriesid").filter(|v| *v > 0),
        episode_id: r.i64("episodeid").filter(|v| *v > 0),
        source_title: r.str("sourcetitle").unwrap_or_default(),
        date: r.ts("date")?,
        event_type: history_event(r.raw("eventtype")),
        quality_id,
        revision,
        languages: language_ids(r, "languages"),
        download_id: r.str("downloadid"),
        data: r
            .json("data")
            .filter(|v| v.is_object())
            .unwrap_or_else(|| Value::Object(Map::new())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(format: SourceFormat, tables: Vec<(&str, Vec<Value>)>) -> RawSource {
        RawSource {
            format,
            tables: tables
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into_iter().map(Rec::from_value).collect()))
                .collect(),
        }
    }

    #[test]
    fn database_and_api_encodings_normalise_identically() {
        let db = raw(
            SourceFormat::Database,
            vec![(
                "series",
                vec![
                    json!({"Id": 3, "Title": "Cowboy Bebop", "Path": "/media/anime/Cowboy Bebop", "TvdbId": 76885, "Status": 1,
                        "SeriesType": 2, "MonitorNewItems": 1, "Monitored": 1, "QualityProfileId": 6, "Tags": "[\n 2\n]",
                        "Seasons": "[{\"seasonNumber\": 1, \"monitored\": false, \"images\": []}]",
                        "FirstAired": "1998-04-03 00:00:00Z", "Added": "2026-11-09 17:28:34.966664Z", "MalIds": "[1, 5]"}),
                ],
            )],
        );
        let api = raw(
            SourceFormat::Api,
            vec![(
                "series",
                vec![
                    json!({"id": 3, "title": "Cowboy Bebop", "path": "/media/anime/Cowboy Bebop", "rootFolderPath": "/media/anime",
                        "tvdbId": 76885, "status": "ended", "seriesType": "anime", "monitorNewItems": "none", "monitored": true,
                        "qualityProfileId": 6, "tags": [2], "seasons": [{"seasonNumber": 1, "monitored": false}],
                        "firstAired": "1998-04-03T00:00:00Z", "added": "2026-11-09T17:28:34Z", "malIds": [1, 5]}),
                ],
            )],
        );
        let a = &normalize(&db).series[0];
        let b = &normalize(&api).series[0];
        assert_eq!(a, b);
        assert_eq!(a.status, SeriesStatus::Ended);
        assert_eq!(a.series_type, SeriesType::Anime);
        assert_eq!(a.monitor_new_items, MonitorNewItems::None);
        assert_eq!(a.root_folder_path.as_deref(), Some("/media/anime"));
        assert_eq!(a.identity(), (MetadataProvider::Tvdb, "76885".to_string()));
    }

    #[test]
    fn tanukarr_identity_and_movies() {
        let snap = normalize(&raw(
            SourceFormat::Database,
            vec![(
                "series",
                vec![
                    json!({"Id": 4, "Title": "Cowboy Bebop: The Movie", "Path": "/media/movies/CB", "TvdbId": -1,
                        "MetadataSource": 2, "ForeignId": "5", "MediaType": 1, "AlternateTitles": "[\"Cowboy Bebop Movie\"]",
                        "AnimeGroupId": "anilist:1", "AnimeGroupTitle": "Cowboy Bebop", "AnimeGroupMemberKind": 1,
                        "AnimeGroupMovieNumber": 1, "OrderingId": "group-123"}),
                ],
            )],
        ));
        let s = &snap.series[0];
        assert_eq!(s.identity(), (MetadataProvider::AniList, "5".to_string()));
        assert_eq!(s.media_type, SrcMediaType::Movie);
        assert_eq!(
            s.alternate_titles,
            vec![("Cowboy Bebop Movie".to_string(), None)]
        );
        assert_eq!(s.group.as_ref().unwrap().position(), 1001);
        assert_eq!(s.unmapped.get("orderingid").unwrap(), "group-123");
    }

    #[test]
    fn custom_format_shapes() {
        let db = custom_format(&Rec::from_value(json!({"Id": 3, "Name": "Bad", "IncludeCustomFormatWhenRenaming": 0,
            "Specifications": "[{\"type\": \"SizeSpecification\", \"body\": {\"order\": 8, \"min\": 0, \"max\": 0.4, \"name\": \"Small\", \"negate\": false, \"required\": true}}]"}))).unwrap();
        let api = custom_format(&Rec::from_value(json!({"id": 3, "name": "Bad", "includeCustomFormatWhenRenaming": false,
            "specifications": [{"name": "Small", "implementation": "SizeSpecification", "negate": false, "required": true,
                                "fields": [{"name": "min", "value": 0}, {"name": "max", "value": 0.4}]}]}))).unwrap();
        assert_eq!(db, api);
        assert_eq!(db.specifications[0].fields.get("max").unwrap(), &json!(0.4));
    }

    #[test]
    fn provider_shapes_and_masking() {
        let api = provider(&Rec::from_value(json!({"id": 1, "name": "q", "implementation": "QBittorrent", "enable": true,
            "priority": 1, "tags": [], "fields": [{"name": "host", "value": "qb"}, {"name": "password", "value": "********", "privacy": "password"}]})),
            ProviderKind::DownloadClient).unwrap();
        assert_eq!(api.masked_fields, vec!["password"]);
        assert_eq!(api.settings.get("host").unwrap(), "qb");
        let db = provider(&Rec::from_value(json!({"Id": 1, "Name": "q", "Implementation": "QBittorrent", "Enable": "True",
            "Settings": "{\"host\": \"qb\", \"password\": \"secret\"}", "ConfigContract": "QBittorrentSettings"})),
            ProviderKind::DownloadClient).unwrap();
        assert!(db.masked_fields.is_empty());
        assert_eq!(db.settings.get("password").unwrap(), "secret");
        assert!(db.enabled);
        let n = provider(&Rec::from_value(json!({"Id": 2, "Name": "hook", "Implementation": "Webhook", "OnGrab": 1, "OnDownload": 0, "Settings": "{}"})),
            ProviderKind::Notification).unwrap();
        assert_eq!(n.options.get("onGrab").unwrap(), true);
        assert_eq!(n.options.get("onDownload").unwrap(), false);
    }

    #[test]
    fn profile_items_both_shapes() {
        let db = quality_profile(&Rec::from_value(json!({"Id": 1, "Name": "HD", "Cutoff": 1000, "UpgradeAllowed": 1,
            "Items": "[{\"quality\": 4, \"items\": [], \"allowed\": true}, {\"id\": 1000, \"name\": \"WEB 720p\", \"items\": [{\"quality\": 14, \"items\": [], \"allowed\": true}, {\"quality\": 5, \"items\": [], \"allowed\": true}], \"allowed\": true}]",
            "FormatItems": "[{\"format\": 2, \"score\": -100}]"}))).unwrap();
        let api = quality_profile(&Rec::from_value(json!({"id": 1, "name": "HD", "cutoff": 1000, "upgradeAllowed": true,
            "items": [{"quality": {"id": 4, "name": "HDTV-720p"}, "items": [], "allowed": true},
                      {"id": 1000, "name": "WEB 720p", "allowed": true, "items": [{"quality": {"id": 14}, "items": [], "allowed": true}, {"quality": {"id": 5}, "items": [], "allowed": true}]}],
            "formatItems": [{"format": 2, "name": "x", "score": -100}]}))).unwrap();
        assert_eq!(db, api);
        assert_eq!(db.items[1].group.as_ref().unwrap().2, vec![14, 5]);
    }

    #[test]
    fn history_and_release_profiles() {
        let h = history(&Rec::from_value(json!({"Id": 1, "EpisodeId": 5, "SeriesId": 2, "SourceTitle": "x", "Date": "2026-01-01 10:00:00Z",
            "Quality": "{\"quality\": 7, \"revision\": {\"version\": 2, \"real\": 0, \"isRepack\": true}}", "EventType": 3,
            "Data": "{\"indexer\": \"Nyaa\"}", "DownloadId": "abc"}))).unwrap();
        assert_eq!(h.event_type, "downloadFolderImported");
        assert_eq!(h.quality_id, Some(7));
        assert!(h.revision.unwrap().is_repack);
        let api = history(&Rec::from_value(json!({"id": 1, "eventType": "grabbed", "date": "2026-01-01T10:00:00Z", "sourceTitle": "x",
            "quality": {"quality": {"id": 3}, "revision": {"version": 1}}}))).unwrap();
        assert_eq!(api.event_type, "grabbed");
        let v3 = release_profile(&Rec::from_value(
            json!({"Id": 1, "Required": "x265,HEVC", "Ignored": "", "IndexerId": 0, "Tags": "[]"}),
        ))
        .unwrap();
        assert_eq!(v3.required, vec!["x265", "HEVC"]);
        assert!(v3.indexer_ids.is_empty());
    }
}
