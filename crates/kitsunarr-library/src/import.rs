//! Importing a completed download into the library: Sonarr's `DownloadedEpisodesImportService`,
//! `ImportDecisionMaker` (the specifications Kitsunarr has data for), the aggregators,
//! `ImportApprovedEpisodes` and `EpisodeFileMovingService`.
//!
//! Safety rules, stricter than upstream where noted:
//! - files are only ever created inside the series folder, and a destination that already
//!   exists is never overwritten;
//! - the new file is transferred *before* a file it upgrades is removed (upstream removes first),
//!   so a failed transfer never loses the existing file;
//! - a replaced file is removed only when its path lies inside the series folder; it goes to the
//!   recycle bin when one is set, else it is deleted (upstream's behaviour);
//! - downloads from a download client are copied (hard-linked when allowed), never moved, so
//!   seeding torrents keep their data.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use kitsunarr_db::history::{self, GrabRecord, NewHistory};
use kitsunarr_db::library::series::{self, NewEpisodeFile};
use kitsunarr_db::{Database, settings};
use kitsunarr_decision::{self as decision, ProfileView};
use kitsunarr_domain::api::settings::{
    MediaManagementSettings, NamingSettings, ProperDownloadType,
};
use kitsunarr_domain::quality::{QualityModel, Revision};
use kitsunarr_domain::{Language, QualitySource};
use kitsunarr_parser::netregex::NetRegex;
use kitsunarr_parser::quality::find_by_source_and_resolution;
use kitsunarr_parser::{
    DetectionSource, ParsedQuality, is_scene_title, parse_languages, parse_path, parse_quality,
    parse_release_group, parse_title,
};

/// `MediaFileExtensions`.
const VIDEO_EXTENSIONS: &[&str] = &[
    ".webm", ".m4v", ".3gp", ".nsv", ".ty", ".strm", ".rm", ".rmvb", ".m3u", ".ifo", ".mov", ".qt",
    ".divx", ".xvid", ".bivx", ".nrg", ".pva", ".wmv", ".asf", ".asx", ".ogm", ".ogv", ".m2v",
    ".avi", ".bin", ".dat", ".dvr-ms", ".mpg", ".mpeg", ".mp4", ".avc", ".vp3", ".svq3", ".nuv",
    ".viv", ".dv", ".fli", ".flv", ".wpl", ".img", ".iso", ".vob", ".mkv", ".ts", ".wtv", ".m2ts",
];

static EXCLUDED_SUBFOLDERS: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"(?:\\|\/|^)(?:@eadir|\.@__thumb|plex versions|\.[^\\/]+)(?:\\|\/)",
        true,
    )
});
static EXCLUDED_EXTRAS_SUBFOLDERS: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"(?:\\|\/|^)(?:extras|extrafanart|behind the scenes|deleted scenes|featurettes|interviews|other|scenes|samples|shorts|trailers|theme[-_. ]music|backdrops)(?:\\|\/)",
        true,
    )
});
static EXCLUDED_FILES: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"^\.(_|unmanic|DS_Store$)|^Thumbs\.db$", true));
static EXCLUDED_EXTRA_FILES: LazyLock<NetRegex> = LazyLock::new(|| {
    NetRegex::compile(
        r"-(trailer|other|behindthescenes|deleted|featurette|interview|scene|short)\.[^.]+$",
        true,
    )
});
/// Kitsunarr cannot measure runtimes yet (Sonarr uses ffprobe), so samples are recognised by
/// name only.
static SAMPLE_NAME: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"\bsample\b", true));

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Recursive video files, without following symbolic links, filtered as `FilterPaths` does.
fn video_files(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && VIDEO_EXTENSIONS.contains(&extension(&path).as_str()) {
                out.push(path);
            }
        }
    }
    out.retain(|p| {
        let relative = p
            .strip_prefix(base)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        !EXCLUDED_SUBFOLDERS.is_match(&relative)
            && !EXCLUDED_EXTRAS_SUBFOLDERS.is_match(&relative)
            && !EXCLUDED_FILES.is_match(&name)
            && !EXCLUDED_EXTRA_FILES.is_match(&name)
    });
    out.sort();
    out
}

/// `FileNameBuilder.CleanFileName` with the default replacement settings.
pub fn clean_file_name(name: &str) -> String {
    let mut s = name.replace(": ", " - ").replace(':', "-");
    s = s
        .chars()
        .filter_map(|c| match c {
            '\\' | '/' => Some('+'),
            '?' => Some('!'),
            '*' => Some('-'),
            '<' | '>' | '|' | '"' => None,
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    s.trim_start_matches([' ', '.']).trim_end().to_string()
}

/// The season folder name for the default tokens (`{season}`, `{season:0}`, `{season:00}`).
/// Other naming tokens need the naming engine, which Kitsunarr does not have yet.
fn season_folder(naming: &NamingSettings, season: i32) -> Result<String, String> {
    let format = if season == 0 {
        &naming.specials_folder_format
    } else {
        &naming.season_folder_format
    };
    let name = format
        .replace("{season:00}", &format!("{season:02}"))
        .replace("{season:0}", &season.to_string())
        .replace("{season}", &season.to_string());
    if name.contains('{') {
        return Err(format!(
            "The season folder format \"{format}\" uses naming tokens Kitsunarr cannot fill in yet; use \"Season {{season}}\" or \"Season {{season:00}}\"."
        ));
    }
    Ok(clean_file_name(&name))
}

/// `true` when `path` is `root` or inside it, compared lexically after normalising `.` and
/// `..` (no symbolic links are followed).
fn is_within(path: &Path, root: &Path) -> bool {
    fn normal(p: &Path) -> Option<PathBuf> {
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    if !out.pop() {
                        return None;
                    }
                }
                Component::CurDir => {}
                other => out.push(other),
            }
        }
        Some(out)
    }
    match (normal(path), normal(root)) {
        (Some(p), Some(r)) => p.starts_with(r),
        _ => false,
    }
}

/// Quality confidence (`Confidence`): from the name, else a fallback (the extension).
fn confidence(d: DetectionSource) -> u8 {
    match d {
        DetectionSource::MediaInfo => 3,
        DetectionSource::Name => 2,
        _ => 0,
    }
}

/// `AggregateQuality`: candidates in augmenter order (file, folder, client title, grab title).
pub fn aggregate_quality(candidates: &[ParsedQuality]) -> QualityModel {
    let (mut source, mut source_conf) = (QualitySource::Unknown, 0u8);
    let (mut resolution, mut resolution_conf) = (0i32, 0u8);
    let (mut revision, mut revision_conf) = (Revision::default(), 1u8);
    for c in candidates {
        let sc = confidence(c.source_detection);
        if source == QualitySource::Unknown
            || (sc > source_conf && c.quality.source != QualitySource::Unknown)
        {
            source = c.quality.source;
            source_conf = sc;
        }
        let rc = confidence(c.resolution_detection);
        if resolution == 0 || (rc > resolution_conf && c.quality.resolution > 0) {
            resolution = c.quality.resolution;
            resolution_conf = rc;
        }
        let vc = if c.revision_detection == DetectionSource::Name {
            2
        } else {
            1
        };
        if vc > revision_conf
            || (vc == revision_conf
                && decision::profile::compare_revision(&c.revision, &revision)
                    == std::cmp::Ordering::Greater)
        {
            revision = c.revision;
            revision_conf = vc;
        }
    }
    QualityModel {
        quality_id: find_by_source_and_resolution(source, resolution).id,
        revision,
    }
}

/// A completed download to import.
#[derive(Debug, Clone)]
pub struct Download<'a> {
    pub download_id: &'a str,
    /// The client's title for the download.
    pub title: &'a str,
    /// Local path of the finished files.
    pub output_path: &'a Path,
    pub series_id: i64,
    /// Its "grabbed" history entries.
    pub grabs: &'a [GrabRecord],
    pub client_name: &'a str,
    pub client_implementation: &'a str,
}

/// What happened to one file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileResult {
    pub path: PathBuf,
    pub imported: bool,
    pub episode_ids: Vec<i64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub results: Vec<FileResult>,
    /// A problem with the whole download (nothing was attempted).
    pub blocked: Option<String>,
}

impl Outcome {
    fn blocked(message: impl Into<String>) -> Self {
        Self {
            results: Vec::new(),
            blocked: Some(message.into()),
        }
    }

    /// Distinct episodes imported.
    pub fn imported_episodes(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.imported)
            .flat_map(|r| r.episode_ids.iter())
            .collect::<HashSet<_>>()
            .len()
    }
}

struct Candidate {
    path: PathBuf,
    size: i64,
    episodes: Vec<i64>,
    season: i32,
    quality: QualityModel,
    languages: Vec<i32>,
    release_group: Option<String>,
    scene_name: Option<String>,
    errors: Vec<String>,
}

fn label(e: &decision::Episode) -> String {
    format!("{}x{:02}", e.season_number, e.episode_number)
}

/// Imports a completed download. Database or filesystem failures of single files are reported
/// per file; only problems with the whole download stop the import.
pub async fn import_download(db: &Database, d: &Download<'_>) -> Result<Outcome, String> {
    let err = |e: kitsunarr_db::DbError| e.to_string();
    let naming: NamingSettings = settings::get_or_default(db.pool(), settings::KEY_NAMING)
        .await
        .map_err(err)?;
    if naming.rename_episodes {
        return Ok(Outcome::blocked(
            "\"Rename files on import\" is on, but Kitsunarr's episode naming is not implemented yet. Turn it off in Settings › Media management to import with the release's own file names.",
        ));
    }
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    let output = d.output_path;
    if !output.exists() {
        return Ok(Outcome::blocked(format!(
            "Import failed, path does not exist or is not accessible by Kitsunarr: {}. Ensure the path exists and the user running Kitsunarr can read it; you may need a remote path mapping.",
            output.display()
        )));
    }
    let detail = series::get_detail(db, d.series_id).await.map_err(err)?;
    let series_path = PathBuf::from(&detail.item.path);
    if output.is_dir() && is_within(&series_path, output) || is_within(output, &series_path) {
        return Ok(Outcome::blocked("Import path is mapped to a series folder"));
    }
    let episodes = series::list_episodes(db, d.series_id, None)
        .await
        .map_err(err)?;
    let model = crate::decision_input::series_model(db, &detail, &episodes)
        .await
        .map_err(err)?;
    let profile = ProfileView(&model.profile);

    let files: Vec<PathBuf> = if output.is_dir() {
        video_files(output)
    } else {
        vec![output.to_path_buf()]
    };
    let folder_name = output
        .is_dir()
        .then(|| output.file_name().map(|n| n.to_string_lossy().into_owned()))
        .flatten();
    let folder_info = folder_name.as_deref().and_then(parse_title);
    let client_info = parse_title(d.title);
    let other_videos = files.len() > 1;
    let grab_episodes: HashSet<i64> = d.grabs.iter().filter_map(|g| g.episode_id).collect();
    let grab_title = d
        .grabs
        .last()
        .map(|g| g.source_title.clone())
        .unwrap_or_default();
    let grab_data: serde_json::Value = d
        .grabs
        .last()
        .and_then(|g| serde_json::from_str(&g.data).ok())
        .unwrap_or_default();
    let unknown = Language::by_id(0).map(|l| l.id).unwrap_or(0);
    let original = model.original_language.map(|l| l.id).unwrap_or(unknown);

    let mut candidates = Vec::new();
    let mut results = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = extension(&path);
        let mut errors = Vec::new();
        if name.starts_with("._") {
            errors.push("Invalid video file, filename starts with '._'".to_string());
        } else if !VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            errors.push(format!(
                "Invalid video file, unsupported extension: '{ext}'"
            ));
        }
        if !errors.is_empty() {
            results.push(FileResult {
                path,
                imported: false,
                episode_ids: Vec::new(),
                errors,
            });
            continue;
        }
        let file_info = parse_path(&path.to_string_lossy());
        // `AggregateEpisodes`: the file's own numbers; a single video not named like a release
        // takes the download's (or its folder's) numbers.
        let mut info = file_info.clone();
        if !other_videos && !is_scene_title(&stem(&path)) {
            if let Some(c) = client_info.as_ref().filter(|c| !c.full_season) {
                info = Some(c.clone());
            } else if let Some(f) = folder_info.as_ref().filter(|f| !f.full_season) {
                info = Some(f.clone());
            }
        }
        let Some(info) = info else {
            results.push(FileResult {
                path,
                imported: false,
                episode_ids: Vec::new(),
                errors: vec!["Unable to parse file".into()],
            });
            continue;
        };
        let mapped: Vec<&decision::Episode> = decision::map_episodes(&info, &model);
        if mapped.is_empty() {
            let message = if info.is_partial_season {
                "Partial season packs are not supported"
            } else if info.is_season_extra {
                "Extras are not supported"
            } else {
                "Invalid season or episode"
            };
            results.push(FileResult {
                path,
                imported: false,
                episode_ids: Vec::new(),
                errors: vec![message.into()],
            });
            continue;
        }
        let size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        let quality = aggregate_quality(&[
            parse_quality(&name),
            folder_name
                .as_deref()
                .map(parse_quality)
                .unwrap_or_else(ParsedQuality::unknown),
            parse_quality(d.title),
            parse_quality(&grab_title),
        ]);
        // `AggregateLanguage`: the last of file, folder and download names that names one.
        let mut languages = vec![original];
        for source in [Some(name.as_str()), folder_name.as_deref(), Some(d.title)]
            .into_iter()
            .flatten()
        {
            let found = parse_languages(source);
            if !found.is_empty() && !(found.len() == 1 && found[0].id == unknown) {
                languages = found.iter().map(|l| l.id).collect();
            }
        }
        // `AggregateReleaseGroup`.
        let client_full = client_info.as_ref().is_some_and(|c| c.full_season);
        let folder_full = folder_info.as_ref().is_some_and(|f| f.full_season);
        let release_group = [
            (!client_full)
                .then(|| parse_release_group(d.title))
                .flatten(),
            (!folder_full)
                .then(|| folder_name.as_deref().and_then(parse_release_group))
                .flatten(),
            parse_release_group(&name),
            parse_release_group(d.title),
            folder_name.as_deref().and_then(parse_release_group),
        ]
        .into_iter()
        .flatten()
        .find(|g| !g.trim().is_empty());
        // `SceneNameCalculator`.
        let scene_name = if !other_videos && !client_full {
            Some(remove_extension(d.title))
        } else if is_scene_title(&stem(&path)) {
            Some(stem(&path))
        } else {
            folder_name
                .clone()
                .filter(|f| !other_videos && !folder_full && is_scene_title(f))
        };

        // Specifications.
        let mut errors = Vec::new();
        if file_info.as_ref().is_some_and(|f| f.full_season) {
            errors.push(
                "Single episode file contains all episodes in seasons. Review file name or manually import".into(),
            );
        }
        if info.is_split_episode {
            errors.push("Single episode split into multiple files".into());
        }
        if SAMPLE_NAME.is_match(&stem(&path)) {
            errors.push("Sample".into());
        }
        let missing: Vec<&decision::Episode> = mapped
            .iter()
            .filter(|e| !grab_episodes.is_empty() && !grab_episodes.contains(&e.id))
            .copied()
            .collect();
        if !missing.is_empty() {
            let labels: Vec<String> = missing.iter().map(|e| label(e)).collect();
            errors.push(if labels.len() == 1 {
                format!(
                    "Episode {} was not found in the grabbed release: {grab_title}",
                    labels[0]
                )
            } else {
                format!(
                    "Episodes {} were not found in the grabbed release: {grab_title}",
                    labels.join(", ")
                )
            });
        }
        let ids: Vec<i64> = mapped.iter().map(|e| e.id).collect();
        for existing in mapped.iter().filter_map(|e| e.file.as_ref()) {
            let order = profile.compare_qualities(quality.quality_id, existing.quality.quality_id);
            if order == std::cmp::Ordering::Less {
                errors.push(format!(
                    "Not an upgrade for existing episode file(s). Existing quality: {}. New Quality {}.",
                    decision::profile::quality_model_label(&existing.quality),
                    decision::profile::quality_model_label(&quality)
                ));
            } else if order == std::cmp::Ordering::Equal
                && media.download_propers_and_repacks != ProperDownloadType::DoNotPrefer
                && decision::profile::compare_revision(
                    &quality.revision,
                    &existing.quality.revision,
                ) == std::cmp::Ordering::Less
            {
                errors.push("Not a quality revision upgrade for existing episode file(s)".into());
            }
            // `SameEpisodesImport`: the existing file holds episodes this one does not.
            if model.episodes.iter().any(|e| {
                e.file.as_ref().is_some_and(|f| f.id == existing.id) && !ids.contains(&e.id)
            }) {
                errors.push(
                    "Episode file on disk contains more episodes than this file contains".into(),
                );
            }
        }
        if !media.skip_free_space_check
            && let Ok(free) = fs4::available_space(&detail.root_folder_path)
            && (free as i64) < size + media.minimum_free_space_mb * 1024 * 1024
        {
            errors.push("Not enough free space".into());
        }
        errors.dedup();
        candidates.push(Candidate {
            path,
            size,
            season: mapped[0].season_number,
            episodes: ids,
            quality,
            languages,
            release_group,
            scene_name,
            errors,
        });
    }

    // `ImportApprovedEpisodes`: approved files by first episode, larger first; an episode is
    // imported once.
    let (mut approved, rejected): (Vec<Candidate>, Vec<Candidate>) =
        candidates.into_iter().partition(|c| c.errors.is_empty());
    let first_number = |c: &Candidate| {
        c.episodes
            .iter()
            .filter_map(|id| model.episodes.iter().find(|e| e.id == *id))
            .map(|e| e.episode_number)
            .min()
            .unwrap_or(0)
    };
    approved.sort_by(|a, b| {
        first_number(a)
            .cmp(&first_number(b))
            .then(b.size.cmp(&a.size))
    });
    let mut done: HashSet<i64> = HashSet::new();
    for c in approved {
        if c.episodes.iter().any(|e| done.contains(e)) {
            results.push(FileResult {
                path: c.path,
                imported: false,
                episode_ids: c.episodes,
                errors: vec!["Episode has already been imported".into()],
            });
            continue;
        }
        done.extend(c.episodes.iter().copied());
        let outcome = transfer_and_record(
            db,
            d,
            &detail,
            &model,
            &media,
            &naming,
            &series_path,
            &c,
            &grab_data,
        )
        .await;
        results.push(FileResult {
            imported: outcome.is_ok(),
            errors: outcome.err().into_iter().collect(),
            path: c.path,
            episode_ids: c.episodes,
        });
    }
    results.extend(rejected.into_iter().map(|c| FileResult {
        path: c.path,
        imported: false,
        episode_ids: c.episodes,
        errors: c.errors,
    }));
    Ok(Outcome {
        results,
        blocked: None,
    })
}

fn remove_extension(title: &str) -> String {
    let p = Path::new(title);
    if VIDEO_EXTENSIONS.contains(&extension(p).as_str()) {
        stem(p)
    } else {
        title.to_string()
    }
}

/// `EpisodeFileMovingService` + `UpgradeMediaFileService` + the history and file records.
#[allow(clippy::too_many_arguments)]
async fn transfer_and_record(
    db: &Database,
    d: &Download<'_>,
    detail: &kitsunarr_domain::api::library::SeriesDetail,
    model: &decision::Series,
    media: &MediaManagementSettings,
    naming: &NamingSettings,
    series_path: &Path,
    c: &Candidate,
    grab_data: &serde_json::Value,
) -> Result<(), String> {
    if !Path::new(&detail.root_folder_path).is_dir() {
        return Err("Failed to import episode, Root folder missing.".into());
    }
    let mut dir = series_path.to_path_buf();
    if detail.season_folder {
        dir.push(season_folder(naming, c.season)?);
    }
    let base = c.scene_name.clone().unwrap_or_else(|| stem(&c.path));
    let file_name = format!(
        "{}{}",
        clean_file_name(&base),
        c.path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default()
    );
    let dest = dir.join(&file_name);
    if !is_within(&dest, series_path) || file_name.is_empty() {
        return Err(
            "Failed to import episode, the destination is outside the series folder.".into(),
        );
    }
    if dest.exists() {
        return Err("Failed to import episode, Destination already exists.".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Failed to import episode due to permission issues.".to_string()
        } else {
            format!("Failed to import episode: {e}")
        }
    })?;
    // Downloads stay in the client (seeding): hard link when allowed, else copy.
    let linked = media.use_hardlinks && std::fs::hard_link(&c.path, &dest).is_ok();
    if !linked {
        std::fs::copy(&c.path, &dest).map_err(|e| {
            let _ = std::fs::remove_file(&dest);
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Failed to import episode due to permission issues.".to_string()
            } else {
                format!("Failed to import episode: {e}")
            }
        })?;
    }

    let mut conn = db.pool().acquire().await.map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // Replaced files: after the new one is in place, and only inside the series folder.
    let replaced: Vec<&decision::EpisodeFile> = {
        let mut seen = HashSet::new();
        c.episodes
            .iter()
            .filter_map(|id| model.episodes.iter().find(|e| e.id == *id))
            .filter_map(|e| e.file.as_ref())
            .filter(|f| seen.insert(f.id))
            .collect()
    };
    for old in replaced {
        let record = series::file_records(db, model.id)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|f| f.id == old.id);
        let Some(record) = record else { continue };
        let old_path = series_path.join(&record.relative_path);
        if is_within(&old_path, series_path) && old_path != dest && old_path.is_file() {
            remove_or_recycle(&old_path, media, Path::new(&detail.root_folder_path))?;
        }
        let episode_ids: Vec<i64> = model
            .episodes
            .iter()
            .filter(|e| e.file.as_ref().is_some_and(|f| f.id == old.id))
            .map(|e| e.id)
            .collect();
        for episode_id in &episode_ids {
            history::insert(
                &mut conn,
                &NewHistory {
                    event_type: "episodeFileDeleted".into(),
                    date: now.clone(),
                    source_title: old_path.to_string_lossy().into_owned(),
                    series_id: Some(model.id),
                    episode_id: Some(*episode_id),
                    movie_id: None,
                    quality_id: Some(old.quality.quality_id),
                    revision: serde_json::to_value(old.quality.revision).ok(),
                    languages: old.languages.iter().map(|l| l.id).collect(),
                    custom_format_score: None,
                    download_id: None,
                    data: serde_json::json!({
                        "reason": "Upgrade",
                        "releaseGroup": old.release_group,
                        "size": old.size.to_string(),
                    }),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        series::delete_episode_file(&mut conn, old.id)
            .await
            .map_err(|e| e.to_string())?;
    }

    let relative = dest
        .strip_prefix(series_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| file_name.clone());
    let original_path = d
        .output_path
        .parent()
        .and_then(|parent| c.path.strip_prefix(parent).ok())
        .map(|p| p.to_string_lossy().into_owned());
    let size = std::fs::metadata(&dest)
        .map(|m| m.len() as i64)
        .unwrap_or(c.size);
    let indexer_flags = grab_data
        .get("indexerFlags")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let release_type = grab_data
        .get("releaseType")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let file_id = series::upsert_episode_file(
        &mut conn,
        &NewEpisodeFile {
            series_id: model.id,
            season_number: c.season,
            relative_path: relative,
            size,
            added_at: now.clone(),
            scene_name: c.scene_name.clone(),
            release_group: c.release_group.clone(),
            quality_id: c.quality.quality_id,
            revision_version: c.quality.revision.version,
            revision_real: c.quality.revision.real,
            is_repack: c.quality.revision.is_repack,
            languages: c.languages.clone(),
            media_info: None,
            original_path,
            indexer_flags,
            release_type,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    series::link_episodes_to_file(&mut conn, &c.episodes, file_id)
        .await
        .map_err(|e| e.to_string())?;
    let source_title = c.scene_name.clone().unwrap_or_else(|| stem(&c.path));
    for episode_id in &c.episodes {
        history::insert(
            &mut conn,
            &NewHistory {
                event_type: "downloadFolderImported".into(),
                date: now.clone(),
                source_title: source_title.clone(),
                series_id: Some(model.id),
                episode_id: Some(*episode_id),
                movie_id: None,
                quality_id: Some(c.quality.quality_id),
                revision: serde_json::to_value(c.quality.revision).ok(),
                languages: c.languages.clone(),
                custom_format_score: None,
                download_id: Some(d.download_id.to_string()),
                data: serde_json::json!({
                    "fileId": file_id.to_string(),
                    "droppedPath": c.path.to_string_lossy(),
                    "importedPath": dest.to_string_lossy(),
                    "downloadClient": d.client_implementation,
                    "downloadClientName": d.client_name,
                    "releaseGroup": c.release_group,
                    "size": size.to_string(),
                    "indexerFlags": indexer_flags.to_string(),
                    "releaseType": release_type.to_string(),
                }),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    tracing::info!(file = %dest.display(), linked, "episode imported");
    Ok(())
}

/// `RecycleBinProvider.DeleteFile`: into `<recycle bin>/<path relative to the root folder>`
/// with `_2`, `_3`… on collisions, or deleted when no recycle bin is set.
fn remove_or_recycle(
    path: &Path,
    media: &MediaManagementSettings,
    root: &Path,
) -> Result<(), String> {
    let failed = |e: std::io::Error| {
        format!("Failed to import episode, unable to move existing file to the Recycle Bin. ({e})")
    };
    match media
        .recycle_bin
        .as_deref()
        .filter(|b| !b.trim().is_empty())
    {
        None => std::fs::remove_file(path).map_err(failed),
        Some(bin) => {
            let relative = path
                .parent()
                .and_then(|p| p.strip_prefix(root).ok())
                .unwrap_or(Path::new(""));
            let dir = Path::new(bin).join(relative);
            std::fs::create_dir_all(&dir).map_err(failed)?;
            let name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
            let mut target = dir.join(&name);
            let mut n = 2;
            while target.exists() {
                target = dir.join(format!(
                    "{}_{n}{}",
                    stem(path),
                    path.extension()
                        .map(|e| format!(".{}", e.to_string_lossy()))
                        .unwrap_or_default()
                ));
                n += 1;
            }
            if std::fs::rename(path, &target).is_err() {
                std::fs::copy(path, &target).map_err(failed)?;
                std::fs::remove_file(path).map_err(failed)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_file_names_like_upstream() {
        assert_eq!(clean_file_name("Show: The Movie?"), "Show - The Movie!");
        assert_eq!(clean_file_name(" .Show/Part*1<>|\""), "Show+Part-1");
    }

    #[test]
    fn paths_stay_inside_the_series_folder() {
        let series = Path::new("/tv/Show");
        assert!(is_within(Path::new("/tv/Show/Season 1/a.mkv"), series));
        assert!(!is_within(Path::new("/tv/Show/../Other/a.mkv"), series));
        assert!(!is_within(Path::new("/tv/Shower/a.mkv"), series));
    }

    #[test]
    fn replaced_files_go_to_the_recycle_bin_or_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tv");
        let season = root.join("Show/Season 1");
        std::fs::create_dir_all(&season).unwrap();
        let bin = dir.path().join("bin");
        let mut media = MediaManagementSettings {
            recycle_bin: Some(bin.display().to_string()),
            ..Default::default()
        };
        for round in 0..2 {
            let old = season.join("Show.S01E01.mkv");
            std::fs::write(&old, format!("v{round}")).unwrap();
            remove_or_recycle(&old, &media, &root).unwrap();
            assert!(!old.exists());
        }
        // Kept under the path relative to the root folder; the second gets a suffix.
        assert_eq!(
            std::fs::read(bin.join("Show/Season 1/Show.S01E01.mkv")).unwrap(),
            b"v0"
        );
        assert_eq!(
            std::fs::read(bin.join("Show/Season 1/Show.S01E01_2.mkv")).unwrap(),
            b"v1"
        );
        media.recycle_bin = None;
        let old = season.join("Show.S01E02.mkv");
        std::fs::write(&old, "x").unwrap();
        remove_or_recycle(&old, &media, &root).unwrap();
        assert!(!old.exists());
    }

    #[test]
    fn season_folders_use_the_default_tokens() {
        let mut naming = NamingSettings::default();
        assert_eq!(season_folder(&naming, 3).unwrap(), "Season 3");
        naming.season_folder_format = "Season {season:00}".into();
        assert_eq!(season_folder(&naming, 3).unwrap(), "Season 03");
        assert_eq!(season_folder(&naming, 0).unwrap(), "Specials");
        naming.season_folder_format = "{Series Title} S{season:00}".into();
        assert!(season_folder(&naming, 1).is_err());
    }

    #[test]
    fn quality_follows_the_confidence_rules() {
        // The file name only knows the extension; the release name is explicit.
        let q = aggregate_quality(&[
            parse_quality("Show.S01E01.mkv"),
            ParsedQuality::unknown(),
            parse_quality("Show.S01E01.1080p.BluRay.x264-GRP"),
        ]);
        assert_eq!(
            kitsunarr_domain::Quality::by_id(q.quality_id).unwrap().name,
            "Bluray-1080p"
        );
        // A named file wins over the release.
        let q = aggregate_quality(&[
            parse_quality("Show.S01E01.720p.HDTV.x264-GRP.mkv"),
            parse_quality("Show.S01E01.1080p.BluRay.x264-GRP"),
        ]);
        assert_eq!(
            kitsunarr_domain::Quality::by_id(q.quality_id).unwrap().name,
            "HDTV-720p"
        );
    }

    #[test]
    fn video_files_skip_samples_folders_and_extras() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for f in [
            "Show.S01E01.mkv",
            "Sample/sample.mkv",
            "Samples/x.mkv",
            "Featurettes/x.mkv",
            ".hidden/y.mkv",
            "Show.S01E01-trailer.mkv",
            "notes.txt",
            "sub/Show.S01E02.mp4",
        ] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let found: Vec<String> = video_files(root)
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect();
        // `Sample/` is not an extras folder upstream (only `samples/` is): the file is listed
        // and later rejected as a sample.
        assert_eq!(
            found,
            vec![
                "Sample/sample.mkv",
                "Show.S01E01.mkv",
                "sub/Show.S01E02.mp4"
            ]
        );
        assert!(SAMPLE_NAME.is_match("sample"));
    }
}
