//! Sonarr's metadata consumers (`Extras/Metadata`): files for media centres written next to
//! the media — Kodi (XBMC) / Emby (`tvshow.nfo`, `<episode>.nfo`, artwork), Roksbox and WDTV
//! (episode XML and artwork) and Plex (`.plexmatch`).
//!
//! As in `MetadataService`, every written file is recorded (`metadata_files`) with the
//! SHA-256 of its contents, so unchanged metadata is not rewritten and images are only fetched
//! when missing; files of an episode file follow its renames and go with it when it is
//! replaced or deleted (see `extras::remove_for_file`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use kitsunarr_db::Database;
use kitsunarr_db::settings;
use kitsunarr_domain::api::settings::{
    KodiMetadataSettings, MetadataConsumersSettings, PlexMetadataSettings, XmlMetadataSettings,
};
use kitsunarr_domain::{MediaImage, SeriesStatus};
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::media_info::{AudioStream, HdrFormat, MediaInfo};

static HTTP: OnceLock<reqwest::Client> = OnceLock::new();

/// The server's HTTP client (with the configured proxy) for downloading artwork.
pub fn install_http_client(client: reqwest::Client) {
    let _ = HTTP.set(client);
}

fn http() -> reqwest::Client {
    HTTP.get().cloned().unwrap_or_default()
}

/// The four consumers, by Sonarr's type name (`MetadataFiles.Consumer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumer {
    Kodi(KodiMetadataSettings),
    Roksbox(XmlMetadataSettings),
    Wdtv(XmlMetadataSettings),
    Plex(PlexMetadataSettings),
}

impl Consumer {
    pub fn type_name(&self) -> &'static str {
        match self {
            Consumer::Kodi(_) => "XbmcMetadata",
            Consumer::Roksbox(_) => "RoksboxMetadata",
            Consumer::Wdtv(_) => "WdtvMetadata",
            Consumer::Plex(_) => "PlexMetadata",
        }
    }

    /// The enabled consumers (`MetadataFactory.Enabled`).
    pub fn enabled(s: &MetadataConsumersSettings) -> Vec<Consumer> {
        let mut out = Vec::new();
        if s.kodi.enable {
            out.push(Consumer::Kodi(s.kodi));
        }
        if s.wdtv.enable {
            out.push(Consumer::Wdtv(s.wdtv));
        }
        if s.roksbox.enable {
            out.push(Consumer::Roksbox(s.roksbox));
        }
        if s.plex.enable {
            out.push(Consumer::Plex(s.plex));
        }
        out
    }

    /// Every consumer, enabled or not (`GetAvailableProviders`, used for renames).
    pub fn all(s: &MetadataConsumersSettings) -> [Consumer; 4] {
        [
            Consumer::Kodi(s.kodi),
            Consumer::Wdtv(s.wdtv),
            Consumer::Roksbox(s.roksbox),
            Consumer::Plex(s.plex),
        ]
    }
}

/// Sonarr's `MetadataType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataType {
    SeriesMetadata = 1,
    EpisodeMetadata = 2,
    SeriesImage = 3,
    SeasonImage = 4,
    EpisodeImage = 5,
}

/// `SeriesMetadataReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Scan,
    EpisodeFolderCreated,
    EpisodesImported,
}

/// What the consumers describe of a series.
#[derive(Debug, Clone, Default)]
pub struct SeriesData {
    pub id: i64,
    pub title: String,
    pub year: i32,
    pub path: PathBuf,
    pub overview: Option<String>,
    pub certification: Option<String>,
    pub network: Option<String>,
    pub status: Option<SeriesStatus>,
    pub first_aired: Option<String>,
    pub last_aired: Option<String>,
    pub runtime: i32,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    /// `(value, votes)`.
    pub rating: Option<(f64, i64)>,
    pub tvdb_id: i64,
    pub tmdb_id: i64,
    pub tvmaze_id: i64,
    pub imdb_id: Option<String>,
    pub images: Vec<MediaImage>,
    pub seasons: Vec<(i32, Vec<MediaImage>)>,
}

#[derive(Debug, Clone, Default)]
pub struct EpisodeData {
    pub id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
    pub air_date: Option<String>,
    pub overview: Option<String>,
    pub screenshot: Option<String>,
    pub tvdb_id: i64,
}

#[derive(Debug, Clone, Default)]
pub struct FileData {
    pub id: i64,
    /// Relative to the series folder, with `/` separators.
    pub relative_path: String,
    pub season_number: i32,
    pub media_info: Option<MediaInfo>,
    pub episodes: Vec<EpisodeData>,
}

/// A metadata file to write: path relative to the series folder, contents.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataResult {
    pub relative_path: String,
    pub contents: String,
}

/// An image to fetch: path relative to the series folder, source URL.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageResult {
    pub relative_path: String,
    pub url: String,
}

// ---------------------------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------------------------

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// An element as .NET's `XElement` writes it: `None` content is the empty element
/// (`<plot />`), `Some("")` an empty pair (`<plot></plot>`).
#[derive(Debug, Clone)]
struct El {
    name: &'static str,
    attrs: Vec<(&'static str, String)>,
    text: Option<String>,
    children: Vec<El>,
}

impl El {
    fn new(name: &'static str) -> Self {
        El {
            name,
            attrs: Vec::new(),
            text: None,
            children: Vec::new(),
        }
    }

    fn text(name: &'static str, text: impl Into<Option<String>>) -> Self {
        El {
            text: text.into(),
            ..El::new(name)
        }
    }

    fn attr(mut self, k: &'static str, v: impl ToString) -> Self {
        self.attrs.push((k, v.to_string()));
        self
    }

    fn add(&mut self, child: El) {
        self.children.push(child);
    }

    /// `XmlWriter` with `Indent = true` (two spaces), or `XNode.ToString()`.
    fn write(&self, out: &mut String, depth: usize, indent: bool) {
        if indent {
            out.push_str(&"  ".repeat(depth));
        }
        out.push('<');
        out.push_str(self.name);
        for (k, v) in &self.attrs {
            out.push_str(&format!(" {k}=\"{}\"", escape(v).replace('"', "&quot;")));
        }
        if self.children.is_empty() {
            match &self.text {
                None => out.push_str(" />"),
                Some(t) => {
                    out.push('>');
                    out.push_str(&escape(t));
                    out.push_str(&format!("</{}>", self.name));
                }
            }
        } else {
            out.push('>');
            for c in &self.children {
                if indent {
                    out.push('\n');
                }
                c.write(out, depth + 1, indent);
            }
            if indent {
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
            }
            out.push_str(&format!("</{}>", self.name));
        }
    }

    fn to_string_indented(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, 0, true);
        s
    }
}

/// .NET's shortest round-trip formatting of a double (`8.9`, `9`, `47.016666666666666`).
fn num(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn status_name(s: Option<SeriesStatus>) -> &'static str {
    match s {
        Some(SeriesStatus::Ended) => "Ended",
        Some(SeriesStatus::Upcoming) => "Upcoming",
        Some(SeriesStatus::Deleted) => "Deleted",
        _ => "Continuing",
    }
}

/// `Path.ChangeExtension`.
fn change_extension(path: &str, extension: &str) -> String {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    };
    if extension.is_empty() {
        format!("{dir}{stem}.")
    } else {
        format!("{dir}{stem}.{}", extension.trim_start_matches('.'))
    }
}

fn url_extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 5 => name[i..].to_ascii_lowercase(),
        _ => ".jpg".into(),
    }
}

// ---------------------------------------------------------------------------------------------
// Kodi (XBMC) / Emby
// ---------------------------------------------------------------------------------------------

/// `XbmcMetadataFormatter.FormatAudioCodec`.
pub fn kodi_audio_codec(stream: &AudioStream) -> String {
    let Some(format) = stream.format.as_deref() else {
        return String::new();
    };
    let profile = stream.profile.as_deref().unwrap_or_default();
    match (format, profile) {
        ("dts", "DTS-HD HRA") => "dtshd_hra",
        ("dts", "DTS-HD MA") => "dtshd_ma",
        ("dts", "DTS-HD MA + DTS:X") => "dtshd_ma_x",
        ("dts", "DTS-HD MA + DTS:X IMAX") => "dtshd_ma_x_imax",
        ("truehd", "Dolby TrueHD + Dolby Atmos") => "truehd_atmos",
        ("eac3", "Dolby Digital Plus + Dolby Atmos") => "eac3_ddp_atmos",
        ("aac", "LC") => "aac_lc",
        ("aac", "HE-AAC") => "he_aac",
        ("aac", "HE-AACv2") => "he_aac_v2",
        ("aac", "SSR") => "aac_ssr",
        ("aac", "LTP") => "aac_ltp",
        _ => format,
    }
    .to_string()
}

fn kodi_series(
    s: &KodiMetadataSettings,
    series: &SeriesData,
    reason: Reason,
) -> Option<MetadataResult> {
    if reason == Reason::EpisodesImported {
        return None;
    }
    let mut xml = String::new();
    if s.series_metadata {
        let mut show = El::new("tvshow");
        show.add(El::text("title", series.title.clone()));
        if let Some((value, votes)) = series.rating
            && votes > 0
        {
            show.add(El::text("rating", num(value)));
        }
        show.add(El::text("plot", series.overview.clone()));
        show.add(El::text("mpaa", series.certification.clone()));
        show.add(El::text("id", series.tvdb_id.to_string()));
        show.add(
            El::text("uniqueid", series.tvdb_id.to_string())
                .attr("type", "tvdb")
                .attr("default", "true"),
        );
        if let Some(imdb) = series.imdb_id.as_deref().filter(|i| !i.trim().is_empty()) {
            show.add(El::text("uniqueid", imdb.to_string()).attr("type", "imdb"));
        }
        if series.tmdb_id > 0 {
            show.add(El::text("uniqueid", series.tmdb_id.to_string()).attr("type", "tmdb"));
        }
        if series.tvmaze_id > 0 {
            show.add(El::text("uniqueid", series.tvmaze_id.to_string()).attr("type", "tvmaze"));
        }
        for genre in &series.genres {
            show.add(El::text("genre", genre.clone()));
        }
        for tag in &series.tags {
            show.add(El::text("tag", tag.clone()));
        }
        show.add(El::text("status", status_name(series.status).to_string()));
        if let Some(first) = series.first_aired.as_deref().and_then(date) {
            show.add(El::text("premiered", first));
        }
        // Jellyfin's "enddate".
        if series.status == Some(SeriesStatus::Ended)
            && let Some(last) = series.last_aired.as_deref().and_then(date)
        {
            show.add(El::text("enddate", last));
        }
        show.add(El::text("studio", series.network.clone()));
        if s.series_metadata_episode_guide {
            // `KodiEpisodeGuide` in declaration order, nulls left out.
            let mut guide = vec![("tvdb", series.tvdb_id.to_string())];
            if series.tvmaze_id > 0 {
                guide.push(("tvmaze", series.tvmaze_id.to_string()));
            }
            if series.tmdb_id > 0 {
                guide.push(("tmdb", series.tmdb_id.to_string()));
            }
            if let Some(imdb) = &series.imdb_id {
                guide.push(("imdb", imdb.clone()));
            }
            let json = guide
                .iter()
                .map(|(k, v)| format!("\"{k}\":{}", serde_json::Value::from(v.as_str())))
                .collect::<Vec<_>>()
                .join(",");
            show.add(El::text("episodeguide", format!("{{{json}}}")));
        }
        xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n");
        xml.push_str(&show.to_string_indented());
    }
    if s.series_metadata_url {
        if s.series_metadata {
            xml.push('\n');
        }
        xml.push_str(&format!(
            "https://www.thetvdb.com/?tab=series&id={}",
            series.tvdb_id
        ));
    }
    (!xml.trim().is_empty()).then(|| MetadataResult {
        relative_path: "tvshow.nfo".into(),
        contents: xml,
    })
}

/// `yyyy-MM-dd` of a stored date or timestamp.
fn date(v: &str) -> Option<String> {
    (v.len() >= 10 && v.as_bytes()[4] == b'-').then(|| v[..10].to_string())
}

fn kodi_episode(
    s: &KodiMetadataSettings,
    file: &FileData,
    watched: bool,
) -> Option<MetadataResult> {
    if !s.episode_metadata {
        return None;
    }
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    for episode in &file.episodes {
        let mut d = El::new("episodedetails");
        d.add(El::text("title", episode.title.clone()));
        d.add(El::text("season", episode.season_number.to_string()));
        d.add(El::text("episode", episode.episode_number.to_string()));
        d.add(El::text("aired", episode.air_date.clone()));
        d.add(El::text("plot", episode.overview.clone()));
        d.add(
            El::text("uniqueid", episode.tvdb_id.to_string())
                .attr("type", "tvdb")
                .attr("default", "true"),
        );
        d.add(El::text("uniqueid", episode.id.to_string()).attr("type", "sonarr"));
        match &episode.screenshot {
            None => d.add(El::new("thumb")),
            Some(url) if s.episode_image_thumb => d.add(El::text("thumb", url.clone())),
            Some(_) => {}
        }
        d.add(El::text("watched", watched.to_string()));
        if let Some(mi) = &file.media_info {
            let mut video = El::new("video");
            video.add(El::text(
                "aspect",
                format!("{}", mi.width as f32 / mi.height as f32),
            ));
            video.add(El::text("bitrate", mi.video_bitrate.to_string()));
            video.add(El::text("codec", mi.video_format.clone()));
            video.add(El::text("framerate", fps(mi.video_fps)));
            video.add(El::text("height", mi.height.to_string()));
            video.add(El::text("scantype", mi.scan_type.clone()));
            video.add(El::text("width", mi.width.to_string()));
            video.add(El::text("duration", num(mi.run_time / 60.0)));
            video.add(El::text(
                "durationinseconds",
                num(mi.run_time.round_ties_even()),
            ));
            let hdr = match mi.video_hdr_format {
                HdrFormat::DolbyVision
                | HdrFormat::DolbyVisionHdr10
                | HdrFormat::DolbyVisionHdr10Plus
                | HdrFormat::DolbyVisionHlg
                | HdrFormat::DolbyVisionSdr => Some("dolbyvision"),
                HdrFormat::Hdr10 | HdrFormat::Hdr10Plus | HdrFormat::Pq10 => Some("hdr10"),
                HdrFormat::Hlg10 => Some("hlg"),
                HdrFormat::None => Some(""),
                HdrFormat::UnknownHdr => None,
            };
            if let Some(h) = hdr {
                video.add(El::text("hdrtype", h.to_string()));
            }
            let mut details = El::new("streamdetails");
            details.add(video);
            for a in &mi.audio_streams {
                let mut audio = El::new("audio");
                audio.add(El::text("bitrate", a.bitrate.to_string()));
                audio.add(El::text("channels", a.channels.to_string()));
                audio.add(El::text("codec", kodi_audio_codec(a)));
                audio.add(El::text("language", a.language.clone()));
                details.add(audio);
            }
            for sub in &mi.subtitle_streams {
                let mut subtitle = El::new("subtitle");
                subtitle.add(El::text("language", sub.language.clone()));
                details.add(subtitle);
            }
            let mut info = El::new("fileinfo");
            info.add(details);
            d.add(info);
        }
        xml.push('\n');
        xml.push_str(&d.to_string_indented());
    }
    Some(MetadataResult {
        relative_path: change_extension(&file.relative_path, "nfo"),
        contents: xml.trim_matches('\n').to_string(),
    })
}

/// `decimal.ToString("0.###")`.
fn fps(v: f64) -> String {
    let s = format!("{:.3}", (v * 1000.0).round() / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn kodi_episode_image_name(relative: &str) -> String {
    format!(
        "{}-thumb.jpg",
        change_extension(relative, "").trim_end_matches('.')
    )
}

// ---------------------------------------------------------------------------------------------
// Roksbox and WDTV
// ---------------------------------------------------------------------------------------------

const ROKSBOX_CERTIFICATIONS: &[&str] = &[
    "G", "NC-17", "PG", "PG-13", "R", "UR", "UNRATED", "NR", "TV-Y", "TV-Y7", "TV-Y7-FV", "TV-G",
    "TV-PG", "TV-14", "TV-MA",
];

fn roksbox_episode(
    s: &XmlMetadataSettings,
    series: &SeriesData,
    file: &FileData,
) -> Option<MetadataResult> {
    if !s.episode_metadata {
        return None;
    }
    let mut out = Vec::new();
    for e in &file.episodes {
        let mut v = El::new("video");
        v.add(El::text(
            "title",
            format!(
                "{} - {}x{} - {}",
                series.title,
                e.season_number,
                e.episode_number,
                e.title.clone().unwrap_or_default()
            ),
        ));
        v.add(El::text("year", e.air_date.clone()));
        v.add(El::text("genre", series.genres.join(" / ")));
        // Kitsunarr keeps no cast list.
        v.add(El::text("actors", String::new()));
        v.add(El::text("description", e.overview.clone()));
        v.add(El::text("length", series.runtime.to_string()));
        let mpaa = series
            .certification
            .as_deref()
            .map(str::to_uppercase)
            .filter(|c| ROKSBOX_CERTIFICATIONS.contains(&c.as_str()))
            .unwrap_or_else(|| "UNRATED".into());
        v.add(El::text("mpaa", mpaa));
        out.push(v.to_string_indented());
    }
    Some(MetadataResult {
        relative_path: change_extension(&file.relative_path, "xml"),
        contents: out.join("\n"),
    })
}

fn wdtv_episode(
    s: &XmlMetadataSettings,
    series: &SeriesData,
    file: &FileData,
) -> Option<MetadataResult> {
    if !s.episode_metadata {
        return None;
    }
    let mut out = Vec::new();
    for e in &file.episodes {
        let mut d = El::new("details");
        d.add(El::text("id", series.id.to_string()));
        d.add(El::text(
            "title",
            format!(
                "{} - {}x{:02} - {}",
                series.title,
                e.season_number,
                e.episode_number,
                e.title.clone().unwrap_or_default()
            ),
        ));
        d.add(El::text("series_name", series.title.clone()));
        d.add(El::text("episode_name", e.title.clone()));
        d.add(El::text("season_number", format!("{:02}", e.season_number)));
        d.add(El::text(
            "episode_number",
            format!("{:02}", e.episode_number),
        ));
        d.add(El::text("firstaired", e.air_date.clone()));
        d.add(El::text("genre", series.genres.join(" / ")));
        d.add(El::text("actor", String::new()));
        d.add(El::text("overview", e.overview.clone()));
        out.push(d.to_string_indented());
    }
    Some(MetadataResult {
        relative_path: change_extension(&file.relative_path, "xml"),
        contents: out.join("\n"),
    })
}

fn season_folder_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^(season (?<season>\d+))|(?<specials>specials)").unwrap())
}

/// `GetSeasonFolders` of Roksbox and WDTV: the series' subfolders named "Season N" (as
/// upstream, a "Specials" folder is matched but never mapped).
fn season_folders(series_path: &Path) -> HashMap<i32, String> {
    let mut map = HashMap::new();
    let Ok(dir) = std::fs::read_dir(series_path) else {
        return map;
    };
    for entry in dir.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(c) = season_folder_regex().captures(&name)
            && let Some(n) = c.name("season").and_then(|m| m.as_str().parse().ok())
        {
            map.insert(n, name);
        }
    }
    map
}

fn first_poster(images: &[MediaImage]) -> Option<&MediaImage> {
    images
        .iter()
        .find(|i| i.cover_type == "poster")
        .or_else(|| images.first())
}

// ---------------------------------------------------------------------------------------------
// Plex
// ---------------------------------------------------------------------------------------------

fn plex_series(
    s: &PlexMetadataSettings,
    series: &SeriesData,
    files: &[FileData],
) -> Option<MetadataResult> {
    if !s.series_plex_match_file {
        return None;
    }
    let mut c = String::new();
    c.push_str(&format!("Title: {}\n", series.title));
    c.push_str(&format!("Year: {}\n", series.year));
    c.push_str(&format!("TvdbId: {}\n", series.tvdb_id));
    c.push_str(&format!(
        "ImdbId: {}\n",
        series.imdb_id.clone().unwrap_or_default()
    ));
    if s.episode_mappings {
        for f in files {
            let Some(first) = f.episodes.first() else {
                continue;
            };
            let format = if f.season_number == 0 {
                format!("SP{:02}", first.episode_number)
            } else {
                format!(
                    "S{:02}{}",
                    f.season_number,
                    f.episodes
                        .iter()
                        .map(|e| format!("E{:02}", e.episode_number))
                        .collect::<Vec<_>>()
                        .join("-")
                )
            };
            c.push_str(&format!("Episode: {format}: {}\n", f.relative_path));
        }
    }
    Some(MetadataResult {
        relative_path: ".plexmatch".into(),
        contents: c,
    })
}

// ---------------------------------------------------------------------------------------------
// The consumer interface (`IMetadata`)
// ---------------------------------------------------------------------------------------------

impl Consumer {
    pub fn series_metadata(
        &self,
        series: &SeriesData,
        files: &[FileData],
        reason: Reason,
    ) -> Option<MetadataResult> {
        match self {
            Consumer::Kodi(s) => kodi_series(s, series, reason),
            Consumer::Plex(s) => plex_series(s, series, files),
            Consumer::Roksbox(_) | Consumer::Wdtv(_) => None,
        }
    }

    pub fn episode_metadata(
        &self,
        series: &SeriesData,
        file: &FileData,
        watched: bool,
    ) -> Option<MetadataResult> {
        match self {
            Consumer::Kodi(s) => kodi_episode(s, file, watched),
            Consumer::Roksbox(s) => roksbox_episode(s, series, file),
            Consumer::Wdtv(s) => wdtv_episode(s, series, file),
            Consumer::Plex(_) => None,
        }
    }

    pub fn series_images(&self, series: &SeriesData) -> Vec<ImageResult> {
        match self {
            Consumer::Kodi(s) if s.series_images => series
                .images
                .iter()
                .filter(|i| i.remote_url.starts_with("http"))
                .map(|i| ImageResult {
                    relative_path: format!(
                        "{}{}",
                        i.cover_type.to_lowercase(),
                        url_extension(&i.remote_url)
                    ),
                    url: i.remote_url.clone(),
                })
                .collect(),
            Consumer::Roksbox(s) | Consumer::Wdtv(s) if s.series_images => {
                let Some(image) = first_poster(&series.images) else {
                    return Vec::new();
                };
                let stem = match self {
                    Consumer::Roksbox(_) => series
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    _ => "folder".into(),
                };
                vec![ImageResult {
                    relative_path: format!("{stem}{}", url_extension(&image.remote_url)),
                    url: image.remote_url.clone(),
                }]
            }
            _ => Vec::new(),
        }
    }

    pub fn season_images(
        &self,
        series: &SeriesData,
        season: i32,
        images: &[MediaImage],
        folders: &HashMap<i32, String>,
    ) -> Vec<ImageResult> {
        match self {
            Consumer::Kodi(s) if s.season_images => images
                .iter()
                .map(|i| ImageResult {
                    relative_path: if season == 0 {
                        format!("season-specials-{}.jpg", i.cover_type.to_lowercase())
                    } else {
                        format!("season{season:02}-{}.jpg", i.cover_type.to_lowercase())
                    },
                    url: i.remote_url.clone(),
                })
                .collect(),
            Consumer::Roksbox(s) | Consumer::Wdtv(s) if s.season_images => {
                let (Some(folder), Some(image)) = (folders.get(&season), first_poster(images))
                else {
                    return Vec::new();
                };
                let name = match self {
                    Consumer::Roksbox(_) => format!("{folder}.jpg"),
                    _ => "folder.jpg".into(),
                };
                let _ = series;
                vec![ImageResult {
                    relative_path: format!("{folder}/{name}"),
                    url: image.remote_url.clone(),
                }]
            }
            _ => Vec::new(),
        }
    }

    pub fn episode_images(&self, file: &FileData) -> Vec<ImageResult> {
        let on = match self {
            Consumer::Kodi(s) => s.episode_images,
            Consumer::Roksbox(s) | Consumer::Wdtv(s) => s.episode_images,
            Consumer::Plex(_) => false,
        };
        let Some(url) = file.episodes.first().and_then(|e| e.screenshot.clone()) else {
            return Vec::new();
        };
        if !on {
            return Vec::new();
        }
        let relative_path = match self {
            Consumer::Kodi(_) => kodi_episode_image_name(&file.relative_path),
            Consumer::Roksbox(_) => change_extension(&file.relative_path, "jpg"),
            _ => change_extension(&file.relative_path, "metathumb"),
        };
        vec![ImageResult { relative_path, url }]
    }

    /// `GetFilenameAfterMove`: where an episode file's metadata goes after a rename (relative
    /// to the series folder).
    pub fn file_name_after_move(
        &self,
        file_relative: &str,
        kind: MetadataType,
        current: &str,
    ) -> String {
        match (self, kind) {
            (Consumer::Kodi(_), MetadataType::EpisodeImage) => {
                kodi_episode_image_name(file_relative)
            }
            (Consumer::Kodi(_), MetadataType::EpisodeMetadata) => {
                change_extension(file_relative, "nfo")
            }
            (Consumer::Roksbox(_), MetadataType::EpisodeImage) => {
                change_extension(file_relative, "jpg")
            }
            (Consumer::Wdtv(_), MetadataType::EpisodeImage) => {
                change_extension(file_relative, "metathumb")
            }
            (Consumer::Roksbox(_) | Consumer::Wdtv(_), MetadataType::EpisodeMetadata) => {
                change_extension(file_relative, "xml")
            }
            (Consumer::Plex(_), _) => {
                let ext = current.rsplit('.').next().unwrap_or_default();
                change_extension(file_relative, ext)
            }
            _ => current.to_string(),
        }
    }

    /// `FindMetadataFile`: whether an existing file in the series folder is this consumer's
    /// (its type and season), for files Kitsunarr did not write itself.
    pub fn find_metadata_file(
        &self,
        relative: &str,
        absolute: &Path,
    ) -> Option<(MetadataType, Option<i32>)> {
        let filename = relative.rsplit('/').next()?;
        let lower = filename.to_ascii_lowercase();
        match self {
            Consumer::Kodi(_) => {
                static SERIES: OnceLock<Regex> = OnceLock::new();
                static SEASON: OnceLock<Regex> = OnceLock::new();
                static EPISODE: OnceLock<Regex> = OnceLock::new();
                let series = SERIES.get_or_init(|| {
                    Regex::new(r"(?i)^(?<type>poster|banner|fanart)\.(?:png|jpg)").unwrap()
                });
                let season = SEASON.get_or_init(|| {
                    Regex::new(
                        r"(?i)^season(?<season>\d{2,}|-all|-specials)-(?<type>poster|banner|fanart)\.(?:png|jpg)",
                    )
                    .unwrap()
                });
                let episode =
                    EPISODE.get_or_init(|| Regex::new(r"(?i)-thumb\.(?:png|jpg)").unwrap());
                if series.is_match(filename) {
                    return Some((MetadataType::SeriesImage, None));
                }
                if let Some(c) = season.captures(filename) {
                    let s = &c["season"];
                    if s.contains("specials") {
                        return Some((MetadataType::SeasonImage, Some(0)));
                    }
                    return s.parse().ok().map(|n| (MetadataType::SeasonImage, Some(n)));
                }
                if episode.is_match(filename) {
                    return Some((MetadataType::EpisodeImage, None));
                }
                if lower == "tvshow.nfo" {
                    return Some((MetadataType::SeriesMetadata, None));
                }
                let parsed = kitsunarr_parser::parse_title(filename)?;
                if !parsed.full_season && lower.ends_with(".nfo") && is_kodi_nfo(absolute) {
                    return Some((MetadataType::EpisodeMetadata, None));
                }
                None
            }
            Consumer::Roksbox(_) | Consumer::Wdtv(_) => {
                let parent = relative
                    .rsplit('/')
                    .nth(1)
                    .map(str::to_string)
                    .or_else(|| {
                        absolute
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                    })
                    .unwrap_or_default();
                let stem = filename.rsplit_once('.').map_or(filename, |(s, _)| s);
                let image_name = match self {
                    Consumer::Roksbox(_) => stem.eq_ignore_ascii_case(&parent),
                    _ => stem.eq_ignore_ascii_case("folder"),
                };
                if image_name {
                    if let Some(c) = season_folder_regex().captures(&parent) {
                        if c.name("specials").is_some() {
                            return Some((MetadataType::SeasonImage, Some(0)));
                        }
                        return c["season"]
                            .parse()
                            .ok()
                            .map(|n| (MetadataType::SeasonImage, Some(n)));
                    }
                    return Some((MetadataType::SeriesImage, None));
                }
                let parsed = kitsunarr_parser::parse_title(filename)?;
                if parsed.full_season {
                    return None;
                }
                if lower.ends_with(".xml") {
                    return Some((MetadataType::EpisodeMetadata, None));
                }
                let image_ext = match self {
                    Consumer::Roksbox(_) => ".jpg",
                    _ => ".metathumb",
                };
                if lower.ends_with(image_ext)
                    && (matches!(self, Consumer::Wdtv(_))
                        || (!stem.ends_with("-thumb") && !parent.eq_ignore_ascii_case("metadata")))
                {
                    return Some((MetadataType::EpisodeImage, None));
                }
                None
            }
            Consumer::Plex(_) => {
                (relative == ".plexmatch").then_some((MetadataType::SeriesMetadata, None))
            }
        }
    }
}

/// `XbmcNfoDetector`: a Kodi NFO holds one of its root elements (files over 10 MB are not read).
fn is_kodi_nfo(path: &Path) -> bool {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) > 10 * 1024 * 1024 {
        return false;
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    [
        "movie",
        "tvshow",
        "episodedetails",
        "artist",
        "album",
        "musicvideo",
    ]
    .iter()
    .any(|t| content.contains(&format!("<{t}>")))
}

// ---------------------------------------------------------------------------------------------
// Films (Kodi): `movie.nfo`, poster and fanart in the film's folder
// ---------------------------------------------------------------------------------------------

/// What `movie.nfo` says about a film, as Radarr's Kodi consumer writes it.
pub struct MovieNfo<'a> {
    pub title: &'a str,
    pub original_title: Option<&'a str>,
    pub sort_title: &'a str,
    pub overview: Option<&'a str>,
    pub certification: Option<&'a str>,
    pub runtime: Option<i32>,
    pub tmdb_id: Option<&'a str>,
    pub imdb_id: Option<&'a str>,
    pub genres: &'a [String],
    pub premiered: Option<&'a str>,
    pub year: Option<i32>,
    pub studio: Option<&'a str>,
    pub images: &'a [MediaImage],
}

pub fn kodi_movie(m: &MovieNfo<'_>) -> String {
    let mut movie = El::new("movie");
    movie.add(El::text("title", m.title.to_string()));
    movie.add(El::text(
        "originaltitle",
        m.original_title.unwrap_or(m.title).to_string(),
    ));
    movie.add(El::text("sorttitle", m.sort_title.to_string()));
    movie.add(El::text("plot", m.overview.map(str::to_string)));
    movie.add(El::text("mpaa", m.certification.map(str::to_string)));
    if let Some(r) = m.runtime.filter(|r| *r > 0) {
        movie.add(El::text("runtime", r.to_string()));
    }
    if let Some(imdb) = m.imdb_id {
        movie.add(El::text("id", imdb.to_string()));
    }
    if let Some(tmdb) = m.tmdb_id {
        movie.add(
            El::text("uniqueid", tmdb.to_string())
                .attr("type", "tmdb")
                .attr("default", "true"),
        );
    }
    if let Some(imdb) = m.imdb_id {
        let el = El::text("uniqueid", imdb.to_string()).attr("type", "imdb");
        movie.add(if m.tmdb_id.is_none() {
            el.attr("default", "true")
        } else {
            el
        });
    }
    for g in m.genres {
        movie.add(El::text("genre", g.clone()));
    }
    if let Some(p) = m.premiered.and_then(date) {
        movie.add(El::text("premiered", p));
    }
    if let Some(y) = m.year {
        movie.add(El::text("year", y.to_string()));
    }
    if let Some(st) = m.studio {
        movie.add(El::text("studio", st.to_string()));
    }
    if let Some(poster) = m.images.iter().find(|i| i.cover_type == "poster") {
        movie.add(El::text("thumb", poster.remote_url.clone()).attr("aspect", "poster"));
    }
    if let Some(fanart) = m.images.iter().find(|i| i.cover_type == "fanart") {
        let mut f = El::new("fanart");
        f.add(El::text("thumb", fanart.remote_url.clone()));
        movie.add(f);
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n{}",
        movie.to_string_indented()
    )
}

/// Writes a film's Kodi files when the Kodi consumer is on (its series-metadata option
/// writes `movie.nfo`, its series-images option the poster and fanart). Called after a film
/// is imported or refreshed; failures are logged.
pub async fn after_movie_change(db: &Database, movie_id: i64) {
    let kodi = consumers(db).await.kodi;
    if !kodi.enable || !(kodi.series_metadata || kodi.series_images) {
        return;
    }
    let Ok(d) = kitsunarr_db::library::movies::get_detail(db, movie_id).await else {
        return;
    };
    let folder = PathBuf::from(&d.item.path);
    if !folder.is_dir() {
        return;
    }
    let tracked: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, relative_path, hash FROM movie_metadata_files WHERE movie_id = ? AND consumer = 'XbmcMetadata'",
    )
    .bind(movie_id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();
    let record = |relative: String, hash: Option<String>, kind: i64| async move {
        let now = kitsunarr_db::now();
        let r = sqlx::query(
            "INSERT INTO movie_metadata_files (movie_id, consumer, type, relative_path, hash, added_at, last_updated) \
             VALUES (?, 'XbmcMetadata', ?, ?, ?, ?, ?) ON CONFLICT (movie_id, relative_path) DO UPDATE SET \
             hash = excluded.hash, last_updated = excluded.last_updated",
        )
        .bind(movie_id)
        .bind(kind)
        .bind(&relative)
        .bind(&hash)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await;
        if let Err(e) = r {
            tracing::warn!(error = %e, path = %relative, "film metadata file not recorded");
        }
    };
    if kodi.series_metadata {
        let original = kitsunarr_db::library::movies::original_title(db, movie_id)
            .await
            .ok()
            .flatten();
        let id_of = |kind: kitsunarr_domain::ExternalIdKind| {
            d.external_ids
                .iter()
                .find(|e| e.kind == kind)
                .map(|e| e.value.as_str())
        };
        let xml = kodi_movie(&MovieNfo {
            title: &d.item.title,
            original_title: original.as_deref(),
            sort_title: &d.item.sort_title,
            overview: d.overview.as_deref(),
            certification: d.certification.as_deref(),
            runtime: d.runtime,
            tmdb_id: id_of(kitsunarr_domain::ExternalIdKind::Tmdb),
            imdb_id: id_of(kitsunarr_domain::ExternalIdKind::Imdb),
            genres: &d.genres,
            premiered: d
                .in_cinemas
                .as_deref()
                .or(d.digital_release.as_deref())
                .or(d.physical_release.as_deref()),
            year: d.item.year,
            studio: d.item.studio.as_deref(),
            images: &d.images,
        });
        let h = hash(&xml);
        let path = folder.join("movie.nfo");
        let unchanged = path.is_file()
            && tracked
                .iter()
                .any(|(_, rel, old)| rel == "movie.nfo" && old.as_deref() == Some(h.as_str()));
        if !unchanged {
            match std::fs::write(&path, &xml) {
                Ok(()) => record("movie.nfo".into(), Some(h), 1).await,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "could not write movie.nfo")
                }
            }
        }
    }
    if kodi.series_images {
        for cover in ["poster", "fanart"] {
            let Some(image) = d.images.iter().find(|i| i.cover_type == cover) else {
                continue;
            };
            let relative = format!("{cover}{}", url_extension(&image.remote_url));
            let path = folder.join(&relative);
            if path.is_file() {
                continue;
            }
            let bytes = match http().get(&image.remote_url).send().await {
                Ok(r) if r.status().is_success() => r.bytes().await.ok(),
                _ => None,
            };
            let Some(bytes) = bytes else {
                tracing::warn!(url = %image.remote_url, "couldn't download film artwork");
                continue;
            };
            if std::fs::write(&path, &bytes).is_ok() {
                record(relative, None, 3).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Loading and writing (`MetadataService`)
// ---------------------------------------------------------------------------------------------

async fn consumers(db: &Database) -> MetadataConsumersSettings {
    settings::get_or_default(db.pool(), settings::KEY_METADATA_CONSUMERS)
        .await
        .unwrap_or_default()
}

async fn load_series(db: &Database, id: i64) -> Option<SeriesData> {
    #[derive(sqlx::FromRow)]
    struct Row {
        title: String,
        year: Option<i32>,
        path: String,
        overview: Option<String>,
        certification: Option<String>,
        network: Option<String>,
        status: String,
        first_aired: Option<String>,
        last_aired: Option<String>,
        runtime: Option<i32>,
        genres: String,
        images: String,
        ratings: Option<String>,
    }
    let r: Row = sqlx::query_as(
        "SELECT title, year, path, overview, certification, network, status, first_aired, last_aired, runtime, genres, images, ratings FROM series WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await
    .ok()??;
    let ext: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, external_id FROM series_external_ids WHERE series_id = ?")
            .bind(id)
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
    let ext_num = |kind: &str| {
        ext.iter()
            .filter(|(k, _)| k == kind)
            .find_map(|(_, v)| v.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let tags: Vec<String> = sqlx::query_scalar(
        "SELECT t.label FROM series_tags st JOIN tags t ON t.id = st.tag_id WHERE st.series_id = ? ORDER BY t.label",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();
    let seasons: Vec<(i32, String)> = sqlx::query_as(
        "SELECT season_number, images FROM seasons WHERE series_id = ? ORDER BY season_number",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();
    let rating = r
        .ratings
        .as_deref()
        .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
        .and_then(|v| Some((v["value"].as_f64()?, v["votes"].as_i64().unwrap_or(0))));
    Some(SeriesData {
        id,
        title: r.title,
        year: r.year.unwrap_or(0),
        path: PathBuf::from(r.path),
        overview: r.overview,
        certification: r.certification,
        network: r.network,
        status: r.status.parse().ok(),
        first_aired: r.first_aired,
        last_aired: r.last_aired,
        runtime: r.runtime.unwrap_or(0),
        genres: serde_json::from_str(&r.genres).unwrap_or_default(),
        tags,
        rating,
        tvdb_id: ext_num("tvdb"),
        tmdb_id: ext_num("tmdb"),
        tvmaze_id: ext_num("tvMaze"),
        imdb_id: ext
            .iter()
            .find(|(k, _)| k == "imdb")
            .map(|(_, v)| v.clone()),
        images: serde_json::from_str(&r.images).unwrap_or_default(),
        seasons: seasons
            .into_iter()
            .map(|(n, i)| (n, serde_json::from_str(&i).unwrap_or_default()))
            .collect(),
    })
}

async fn load_files(db: &Database, series_id: i64, only: Option<i64>) -> Vec<FileData> {
    let provider: Option<String> =
        sqlx::query_scalar("SELECT metadata_provider FROM series WHERE id = ?")
            .bind(series_id)
            .fetch_optional(db.pool())
            .await
            .ok()
            .flatten();
    let tvdb_owned = provider.as_deref() == Some("tvdb");
    let files: Vec<(i64, String, i32, Option<String>)> = sqlx::query_as(
        "SELECT id, relative_path, season_number, media_info FROM episode_files WHERE series_id = ? AND (? IS NULL OR id = ?) ORDER BY id",
    )
    .bind(series_id)
    .bind(only)
    .bind(only)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();
    #[derive(sqlx::FromRow)]
    struct Ep {
        id: i64,
        season_number: i32,
        episode_number: i32,
        title: Option<String>,
        air_date: Option<String>,
        overview: Option<String>,
        screenshot: Option<String>,
        provider_episode_id: Option<String>,
        episode_file_id: i64,
    }
    let episodes: Vec<Ep> = sqlx::query_as(
        "SELECT id, season_number, episode_number, title, air_date, overview, screenshot, provider_episode_id, episode_file_id \
         FROM episodes WHERE series_id = ? AND episode_file_id IS NOT NULL ORDER BY season_number, episode_number",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();
    files
        .into_iter()
        .map(|(id, relative_path, season_number, media_info)| FileData {
            id,
            relative_path: relative_path.replace('\\', "/"),
            season_number,
            media_info: media_info
                .as_deref()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                .and_then(|v| MediaInfo::from_json(&v)),
            episodes: episodes
                .iter()
                .filter(|e| e.episode_file_id == id)
                .map(|e| EpisodeData {
                    id: e.id,
                    season_number: e.season_number,
                    episode_number: e.episode_number,
                    title: e.title.clone(),
                    air_date: e.air_date.clone(),
                    overview: e.overview.clone(),
                    screenshot: e.screenshot.clone(),
                    tvdb_id: if tvdb_owned {
                        e.provider_episode_id
                            .as_deref()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0)
                    } else {
                        0
                    },
                })
                .collect(),
        })
        .collect()
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Tracked {
    id: i64,
    season_number: Option<i32>,
    episode_file_id: Option<i64>,
    consumer: String,
    r#type: i64,
    relative_path: String,
    hash: Option<String>,
}

async fn tracked(db: &Database, series_id: i64) -> Vec<Tracked> {
    sqlx::query_as(
        "SELECT id, season_number, episode_file_id, consumer, type, relative_path, hash FROM metadata_files WHERE series_id = ? ORDER BY id",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default()
}

/// `CleanMetadataFileService.Clean`: forgets records whose file is gone.
async fn clean(db: &Database, series: &SeriesData, files: &mut Vec<Tracked>) {
    let mut gone = Vec::new();
    files.retain(|f| {
        let keep = series.path.join(&f.relative_path).is_file();
        if !keep {
            gone.push(f.id);
        }
        keep
    });
    for id in gone {
        let _ = sqlx::query("DELETE FROM metadata_files WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await;
    }
}

struct Writer<'a> {
    db: &'a Database,
    series: &'a SeriesData,
    existing: Vec<Tracked>,
    file_mode: Option<u32>,
}

fn hash(contents: &str) -> String {
    hex::encode(Sha256::digest(contents.as_bytes()))
}

fn extension_of(relative: &str) -> String {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    match name.rfind('.') {
        Some(i) => name[i..].to_string(),
        None => String::new(),
    }
}

impl Writer<'_> {
    fn full(&self, relative: &str) -> PathBuf {
        self.series.path.join(relative)
    }

    /// `GetMetadataFile`: the first matching record; duplicates are removed (file and record).
    async fn find(&mut self, pred: impl Fn(&Tracked) -> bool) -> Option<Tracked> {
        let matching: Vec<Tracked> = self.existing.iter().filter(|t| pred(t)).cloned().collect();
        for dup in matching.iter().skip(1) {
            let path = self.full(&dup.relative_path);
            if crate::import::is_within(&path, &self.series.path) {
                let _ = std::fs::remove_file(&path);
            }
            let _ = sqlx::query("DELETE FROM metadata_files WHERE id = ?")
                .bind(dup.id)
                .execute(self.db.pool())
                .await;
            self.existing.retain(|t| t.id != dup.id);
        }
        matching.into_iter().next()
    }

    async fn upsert(&self, t: &Tracked) {
        let now = kitsunarr_db::now();
        let ext = extension_of(&t.relative_path);
        let result = if t.id > 0 {
            sqlx::query(
                "UPDATE metadata_files SET relative_path = ?, extension = ?, hash = ?, last_updated = ? WHERE id = ?",
            )
            .bind(&t.relative_path)
            .bind(&ext)
            .bind(&t.hash)
            .bind(&now)
            .bind(t.id)
            .execute(self.db.pool())
            .await
        } else {
            sqlx::query(
                "INSERT INTO metadata_files (series_id, season_number, episode_file_id, consumer, type, relative_path, extension, hash, added_at, last_updated) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(self.series.id)
            .bind(t.season_number)
            .bind(t.episode_file_id)
            .bind(&t.consumer)
            .bind(t.r#type)
            .bind(&t.relative_path)
            .bind(&ext)
            .bind(&t.hash)
            .bind(&now)
            .bind(&now)
            .execute(self.db.pool())
            .await
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, path = %t.relative_path, "metadata file not recorded");
        }
    }

    fn save(&self, relative: &str, contents: &str) -> bool {
        let path = self.full(relative);
        if !crate::import::is_within(&path, &self.series.path) {
            return false;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(&path, contents) {
            Ok(()) => {
                self.permissions(&path);
                true
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not write a metadata file");
                false
            }
        }
    }

    fn permissions(&self, path: &Path) {
        #[cfg(unix)]
        if let Some(mode) = self.file_mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
        }
        #[cfg(not(unix))]
        let _ = (path, self.file_mode);
    }

    async fn download(&self, image: &ImageResult) -> bool {
        let path = self.full(&image.relative_path);
        if !crate::import::is_within(&path, &self.series.path) {
            return false;
        }
        let bytes = match http().get(&image.url).send().await {
            Ok(r) if r.status().is_success() => r.bytes().await.ok(),
            Ok(r) => {
                tracing::warn!(url = %image.url, status = %r.status(), "couldn't download image");
                None
            }
            Err(e) => {
                tracing::warn!(url = %image.url, error = %e, "couldn't download image");
                None
            }
        };
        let Some(bytes) = bytes else { return false };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&path, &bytes).is_err() {
            return false;
        }
        self.permissions(&path);
        true
    }

    async fn series_metadata(&mut self, c: &Consumer, files: &[FileData], reason: Reason) {
        let Some(m) = c.series_metadata(self.series, files, reason) else {
            return;
        };
        let h = hash(&m.contents);
        let name = c.type_name();
        let existing = self
            .find(|t| t.consumer == name && t.r#type == MetadataType::SeriesMetadata as i64)
            .await;
        let mut t = existing.unwrap_or(Tracked {
            id: 0,
            season_number: None,
            episode_file_id: None,
            consumer: name.into(),
            r#type: MetadataType::SeriesMetadata as i64,
            relative_path: m.relative_path.clone(),
            hash: None,
        });
        if t.hash.as_deref() == Some(h.as_str()) && self.full(&m.relative_path).is_file() {
            if t.relative_path != m.relative_path {
                t.relative_path = m.relative_path;
                self.upsert(&t).await;
            }
            return;
        }
        if self.save(&m.relative_path, &m.contents) {
            t.hash = Some(h);
            t.relative_path = m.relative_path;
            self.upsert(&t).await;
        }
    }

    async fn episode_metadata(&mut self, c: &Consumer, file: &FileData) {
        let watched = matches!(c, Consumer::Kodi(_)) && {
            let nfo = self.full(&change_extension(&file.relative_path, "nfo"));
            std::fs::read_to_string(nfo)
                .map(|t| t.contains("<watched>true</watched>"))
                .unwrap_or(false)
        };
        let Some(m) = c.episode_metadata(self.series, file, watched) else {
            return;
        };
        let name = c.type_name();
        let existing = self
            .find(|t| {
                t.consumer == name
                    && t.r#type == MetadataType::EpisodeMetadata as i64
                    && t.episode_file_id == Some(file.id)
            })
            .await;
        let mut t = match existing {
            Some(mut t) => {
                if t.relative_path != m.relative_path {
                    let (from, to) = (self.full(&t.relative_path), self.full(&m.relative_path));
                    if crate::import::is_within(&to, &self.series.path) {
                        let _ = std::fs::rename(&from, &to);
                    }
                    t.relative_path = m.relative_path.clone();
                }
                t
            }
            None => Tracked {
                id: 0,
                season_number: Some(file.season_number),
                episode_file_id: Some(file.id),
                consumer: name.into(),
                r#type: MetadataType::EpisodeMetadata as i64,
                relative_path: m.relative_path.clone(),
                hash: None,
            },
        };
        let h = hash(&m.contents);
        if t.hash.as_deref() == Some(h.as_str()) && self.full(&m.relative_path).is_file() {
            if t.id > 0 {
                self.upsert(&t).await;
            }
            return;
        }
        if self.save(&m.relative_path, &m.contents) {
            t.hash = Some(h);
            self.upsert(&t).await;
        }
    }

    async fn images(
        &mut self,
        c: &Consumer,
        images: Vec<ImageResult>,
        kind: MetadataType,
        season: Option<i32>,
        file: Option<i64>,
    ) {
        let name = c.type_name();
        for image in images {
            if self.full(&image.relative_path).is_file() {
                continue;
            }
            let existing = self
                .find(|t| {
                    t.consumer == name
                        && t.r#type == kind as i64
                        && match kind {
                            MetadataType::EpisodeImage => t.episode_file_id == file,
                            MetadataType::SeasonImage => {
                                t.season_number == season && t.relative_path == image.relative_path
                            }
                            _ => t.relative_path == image.relative_path,
                        }
                })
                .await;
            // An episode image recorded under the file's old name is moved, not fetched again.
            if kind == MetadataType::EpisodeImage
                && let Some(mut t) = existing.clone()
                && t.relative_path != image.relative_path
                && self.full(&t.relative_path).is_file()
            {
                let to = self.full(&image.relative_path);
                if crate::import::is_within(&to, &self.series.path)
                    && std::fs::rename(self.full(&t.relative_path), &to).is_ok()
                {
                    t.relative_path = image.relative_path.clone();
                    self.upsert(&t).await;
                }
                continue;
            }
            if self.download(&image).await {
                let mut t = existing.unwrap_or(Tracked {
                    id: 0,
                    season_number: season,
                    episode_file_id: file,
                    consumer: name.into(),
                    r#type: kind as i64,
                    relative_path: image.relative_path.clone(),
                    hash: None,
                });
                t.relative_path = image.relative_path;
                self.upsert(&t).await;
            }
        }
    }
}

async fn writer<'a>(db: &'a Database, series: &'a SeriesData) -> Writer<'a> {
    let media: kitsunarr_domain::api::settings::MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .unwrap_or_default();
    // `SetFilePermissions`: the folder mode without its execute bits.
    let file_mode = media
        .set_permissions
        .then(|| u32::from_str_radix(media.chmod_folder.trim(), 8).ok())
        .flatten()
        .map(|m| m & !0o111);
    let mut existing = tracked(db, series.id).await;
    clean(db, series, &mut existing).await;
    Writer {
        db,
        series,
        existing,
        file_mode,
    }
}

/// `CreateAfterSeriesScan`: series metadata and artwork, season artwork, and every episode
/// file's metadata and thumbnail. Disk scans and refreshes call it.
pub async fn after_series_scan(db: &Database, series_id: i64) {
    let enabled = Consumer::enabled(&consumers(db).await);
    if enabled.is_empty() {
        return;
    }
    let Some(series) = load_series(db, series_id).await else {
        return;
    };
    if !series.path.is_dir() {
        tracing::info!(series = %series.title, "series folder does not exist, skipping metadata creation");
        return;
    }
    let files = load_files(db, series_id, None).await;
    let folders = season_folders(&series.path);
    let mut w = writer(db, &series).await;
    for c in &enabled {
        w.series_metadata(c, &files, Reason::Scan).await;
        w.images(
            c,
            c.series_images(&series),
            MetadataType::SeriesImage,
            None,
            None,
        )
        .await;
        for (n, images) in &series.seasons {
            let list = c.season_images(&series, *n, images, &folders);
            w.images(c, list, MetadataType::SeasonImage, Some(*n), None)
                .await;
        }
        for f in &files {
            w.episode_metadata(c, f).await;
            w.images(
                c,
                c.episode_images(f),
                MetadataType::EpisodeImage,
                Some(f.season_number),
                Some(f.id),
            )
            .await;
        }
    }
}

/// `CreateAfterEpisodeImport`: the new episode file's metadata and thumbnail, and, when the
/// import created the series or season folder, `CreateAfterEpisodeFolder`.
pub async fn after_episode_import(
    db: &Database,
    series_id: i64,
    episode_file_id: i64,
    series_folder_created: bool,
    season_folder_created: bool,
) {
    let enabled = Consumer::enabled(&consumers(db).await);
    if enabled.is_empty() {
        return;
    }
    let Some(series) = load_series(db, series_id).await else {
        return;
    };
    let files = load_files(db, series_id, None).await;
    let folders = season_folders(&series.path);
    let mut w = writer(db, &series).await;
    for c in &enabled {
        if series_folder_created {
            w.series_metadata(c, &files, Reason::EpisodeFolderCreated)
                .await;
            w.images(
                c,
                c.series_images(&series),
                MetadataType::SeriesImage,
                None,
                None,
            )
            .await;
        }
        if season_folder_created {
            for (n, images) in &series.seasons {
                let list = c.season_images(&series, *n, images, &folders);
                w.images(c, list, MetadataType::SeasonImage, Some(*n), None)
                    .await;
            }
        }
        if let Some(f) = files.iter().find(|f| f.id == episode_file_id) {
            w.episode_metadata(c, f).await;
            w.images(
                c,
                c.episode_images(f),
                MetadataType::EpisodeImage,
                Some(f.season_number),
                Some(f.id),
            )
            .await;
        }
    }
}

/// `CreateAfterEpisodesImported` (after a download's imports): the series metadata (Kodi
/// skips it; Plex's `.plexmatch` lists the new files).
pub async fn after_episodes_imported(db: &Database, series_id: i64) {
    let enabled = Consumer::enabled(&consumers(db).await);
    if enabled.is_empty() {
        return;
    }
    let Some(series) = load_series(db, series_id).await else {
        return;
    };
    if !series.path.is_dir() {
        return;
    }
    let files = load_files(db, series_id, None).await;
    let mut w = writer(db, &series).await;
    for c in &enabled {
        w.series_metadata(c, &files, Reason::EpisodesImported).await;
    }
}

/// `MoveFilesAfterRename`: an episode file's metadata follows its new name (every consumer,
/// enabled or not).
pub async fn move_after_rename(db: &Database, series_id: i64, episode_file_id: i64) {
    let Some(series) = load_series(db, series_id).await else {
        return;
    };
    let Some(file) = load_files(db, series_id, Some(episode_file_id)).await.pop() else {
        return;
    };
    let all = Consumer::all(&consumers(db).await);
    let w = writer(db, &series).await;
    for t in w
        .existing
        .iter()
        .filter(|t| t.episode_file_id == Some(episode_file_id))
    {
        let Some(c) = all.iter().find(|c| c.type_name() == t.consumer) else {
            continue;
        };
        let kind = match t.r#type {
            2 => MetadataType::EpisodeMetadata,
            5 => MetadataType::EpisodeImage,
            _ => continue,
        };
        let new = c.file_name_after_move(&file.relative_path, kind, &t.relative_path);
        if new == t.relative_path {
            continue;
        }
        let (from, to) = (w.full(&t.relative_path), w.full(&new));
        if !crate::import::is_within(&to, &series.path) {
            continue;
        }
        if let Some(dir) = to.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                let mut moved = t.clone();
                moved.relative_path = new;
                w.upsert(&moved).await;
            }
            Err(e) => {
                tracing::warn!(path = %from.display(), error = %e, "unable to move metadata file after rename");
            }
        }
    }
}

/// `ExistingMetadataImporter`: metadata files already in the series folder (written by Sonarr
/// before a migration, or by hand) that any consumer recognises are recorded, so they follow
/// renames and go with their episode file. Episode-level files are matched to the episode file
/// their parsed numbers name; files naming episodes of several files are left alone.
pub async fn import_existing(db: &Database, series_id: i64) {
    let Some(series) = load_series(db, series_id).await else {
        return;
    };
    if !series.path.is_dir() {
        return;
    }
    let all = Consumer::all(&consumers(db).await);
    let mut known: std::collections::HashSet<String> = tracked(db, series_id)
        .await
        .into_iter()
        .map(|t| t.relative_path)
        .collect();
    let extras: Vec<String> =
        sqlx::query_scalar("SELECT relative_path FROM extra_files WHERE series_id = ?")
            .bind(series_id)
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
    known.extend(extras.into_iter().map(|p| p.replace('\\', "/")));
    let episodes: Vec<(i32, i32, Option<i64>)> = sqlx::query_as(
        "SELECT season_number, episode_number, episode_file_id FROM episodes WHERE series_id = ?",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await
    .unwrap_or_default();
    let mut stack = vec![series.path.clone()];
    let mut candidates = Vec::new();
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
            } else if meta.is_file() {
                candidates.push(path);
            }
        }
    }
    let w = writer(db, &series).await;
    for path in candidates {
        let Ok(rel) = path.strip_prefix(&series.path) else {
            continue;
        };
        let relative = rel.to_string_lossy().replace('\\', "/");
        let ext = extension_of(&relative).to_ascii_lowercase();
        if known.contains(&relative)
            || kitsunarr_parser::extensions::is_video_extension(&ext)
            || crate::extras::SUBTITLE_EXTENSIONS.contains(&ext.as_str())
        {
            continue;
        }
        for c in &all {
            let Some((kind, season)) = c.find_metadata_file(&relative, &path) else {
                continue;
            };
            let (mut season_number, mut file_id) = (season, None);
            if matches!(
                kind,
                MetadataType::EpisodeImage | MetadataType::EpisodeMetadata
            ) {
                let Some(parsed) = kitsunarr_parser::parse_path(&path.to_string_lossy()) else {
                    break;
                };
                let s = parsed.season_numbers.first().copied().unwrap_or(1);
                let files: std::collections::HashSet<Option<i64>> = episodes
                    .iter()
                    .filter(|(es, en, _)| *es == s && parsed.episode_numbers.contains(en))
                    .map(|(_, _, f)| *f)
                    .collect();
                if files.len() != 1 {
                    break;
                }
                let Some(Some(f)) = files.into_iter().next() else {
                    break;
                };
                season_number = Some(s);
                file_id = Some(f);
            }
            w.upsert(&Tracked {
                id: 0,
                season_number,
                episode_file_id: file_id,
                consumer: c.type_name().into(),
                r#type: kind as i64,
                relative_path: relative.clone(),
                hash: None,
            })
            .await;
            known.insert(relative.clone());
            break;
        }
    }
}

/// Removes an episode file's metadata files (to the recycle bin when set), with their
/// records — Sonarr's `ExtraFileService` on `EpisodeFileDeletedEvent`.
pub async fn remove_for_file(
    db: &Database,
    media: &kitsunarr_domain::api::settings::MediaManagementSettings,
    series_path: &Path,
    root: &Path,
    episode_file_id: i64,
) {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, relative_path FROM metadata_files WHERE episode_file_id = ?")
            .bind(episode_file_id)
            .fetch_all(db.pool())
            .await
            .unwrap_or_default();
    for (id, relative) in rows {
        let path = series_path.join(&relative);
        if path.is_file()
            && crate::import::is_within(&path, series_path)
            && let Err(e) = crate::import::remove_or_recycle(&path, media, root)
        {
            tracing::warn!(path = %path.display(), error = %e, "could not remove a metadata file");
        }
        let _ = sqlx::query("DELETE FROM metadata_files WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_info::SubtitleStream;

    fn series() -> SeriesData {
        SeriesData {
            id: 7,
            title: "Breaking Bad".into(),
            year: 2008,
            path: PathBuf::from("/tv/Breaking Bad"),
            overview: Some("A chemistry teacher & a former student.".into()),
            certification: Some("tv-ma".into()),
            network: Some("AMC".into()),
            status: Some(SeriesStatus::Ended),
            first_aired: Some("2008-01-20".into()),
            last_aired: Some("2013-09-29T00:00:00Z".into()),
            runtime: 47,
            genres: vec!["Crime".into(), "Drama".into()],
            tags: vec!["hd".into()],
            rating: Some((8.9, 1000)),
            tvdb_id: 81189,
            tmdb_id: 1396,
            tvmaze_id: 169,
            imdb_id: Some("tt0903747".into()),
            images: vec![
                MediaImage {
                    cover_type: "poster".into(),
                    remote_url: "https://artworks.thetvdb.com/banners/posters/81189-10.jpg".into(),
                },
                MediaImage {
                    cover_type: "fanart".into(),
                    remote_url:
                        "https://artworks.thetvdb.com/banners/fanart/original/81189-21.png?x=1"
                            .into(),
                },
            ],
            seasons: vec![],
        }
    }

    fn file() -> FileData {
        FileData {
            id: 3,
            relative_path: "Season 01/Breaking Bad - S01E01 - Pilot.mkv".into(),
            season_number: 1,
            media_info: None,
            episodes: vec![EpisodeData {
                id: 11,
                season_number: 1,
                episode_number: 1,
                title: Some("Pilot".into()),
                air_date: Some("2008-01-20".into()),
                overview: None,
                screenshot: Some(
                    "https://artworks.thetvdb.com/banners/episodes/81189/349232.jpg".into(),
                ),
                tvdb_id: 349232,
            }],
        }
    }

    #[test]
    fn kodi_series_nfo_follows_xbmc_metadata() {
        let s = KodiMetadataSettings {
            series_metadata_episode_guide: true,
            series_metadata_url: true,
            ..Default::default()
        };
        let m = kodi_series(&s, &series(), Reason::Scan).unwrap();
        assert_eq!(m.relative_path, "tvshow.nfo");
        assert_eq!(
            m.contents,
            r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<tvshow>
  <title>Breaking Bad</title>
  <rating>8.9</rating>
  <plot>A chemistry teacher &amp; a former student.</plot>
  <mpaa>tv-ma</mpaa>
  <id>81189</id>
  <uniqueid type="tvdb" default="true">81189</uniqueid>
  <uniqueid type="imdb">tt0903747</uniqueid>
  <uniqueid type="tmdb">1396</uniqueid>
  <uniqueid type="tvmaze">169</uniqueid>
  <genre>Crime</genre>
  <genre>Drama</genre>
  <tag>hd</tag>
  <status>Ended</status>
  <premiered>2008-01-20</premiered>
  <enddate>2013-09-29</enddate>
  <studio>AMC</studio>
  <episodeguide>{"tvdb":"81189","tvmaze":"169","tmdb":"1396","imdb":"tt0903747"}</episodeguide>
</tvshow>
https://www.thetvdb.com/?tab=series&id=81189"#
        );
        // Imports alone never rewrite it.
        assert!(kodi_series(&s, &series(), Reason::EpisodesImported).is_none());
        let off = KodiMetadataSettings {
            series_metadata: false,
            ..Default::default()
        };
        assert!(kodi_series(&off, &series(), Reason::Scan).is_none());
    }

    #[test]
    fn kodi_episode_nfo_carries_stream_details() {
        let mut f = file();
        f.media_info = Some(MediaInfo {
            width: 1920,
            height: 1080,
            video_bitrate: 5_000_000,
            video_format: Some("h264".into()),
            video_fps: 23.976023,
            run_time: 2820.6,
            video_hdr_format: HdrFormat::Hdr10,
            audio_streams: vec![AudioStream {
                language: Some("eng".into()),
                format: Some("aac".into()),
                profile: Some("LC".into()),
                bitrate: 192_000,
                channels: 2,
                ..Default::default()
            }],
            subtitle_streams: vec![SubtitleStream {
                language: Some("eng".into()),
                ..Default::default()
            }],
            ..Default::default()
        });
        let m = kodi_episode(&KodiMetadataSettings::default(), &f, true).unwrap();
        assert_eq!(
            m.relative_path,
            "Season 01/Breaking Bad - S01E01 - Pilot.nfo"
        );
        assert_eq!(
            m.contents,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<episodedetails>
  <title>Pilot</title>
  <season>1</season>
  <episode>1</episode>
  <aired>2008-01-20</aired>
  <plot />
  <uniqueid type="tvdb" default="true">349232</uniqueid>
  <uniqueid type="sonarr">11</uniqueid>
  <watched>true</watched>
  <fileinfo>
    <streamdetails>
      <video>
        <aspect>1.7777778</aspect>
        <bitrate>5000000</bitrate>
        <codec>h264</codec>
        <framerate>23.976</framerate>
        <height>1080</height>
        <scantype />
        <width>1920</width>
        <duration>47.01</duration>
        <durationinseconds>2821</durationinseconds>
        <hdrtype>hdr10</hdrtype>
      </video>
      <audio>
        <bitrate>192000</bitrate>
        <channels>2</channels>
        <codec>aac_lc</codec>
        <language>eng</language>
      </audio>
      <subtitle>
        <language>eng</language>
      </subtitle>
    </streamdetails>
  </fileinfo>
</episodedetails>"#
        );
        // Without a screenshot the thumb is empty; with "image thumbs" it is the URL.
        let thumbs = KodiMetadataSettings {
            episode_image_thumb: true,
            ..Default::default()
        };
        assert!(
            kodi_episode(&thumbs, &file(), false)
                .unwrap()
                .contents
                .contains(
                    "<thumb>https://artworks.thetvdb.com/banners/episodes/81189/349232.jpg</thumb>"
                )
        );
        let mut none = file();
        none.episodes[0].screenshot = None;
        assert!(
            kodi_episode(&KodiMetadataSettings::default(), &none, false)
                .unwrap()
                .contents
                .contains("<thumb />")
        );
    }

    #[test]
    fn audio_codecs_follow_format_audio_codec_fixture() {
        for (format, profile, expected) in [
            ("dts", "DTS-HD MA", "dtshd_ma"),
            ("dts", "DTS-ES", "dts"),
            ("truehd", "Dolby TrueHD + Dolby Atmos", "truehd_atmos"),
            ("eac3", "Dolby Digital Plus + Dolby Atmos", "eac3_ddp_atmos"),
            ("aac", "HE-AACv2", "he_aac_v2"),
            ("ac3", "", "ac3"),
        ] {
            let a = AudioStream {
                format: Some(format.into()),
                profile: Some(profile.into()),
                ..Default::default()
            };
            assert_eq!(kodi_audio_codec(&a), expected);
        }
        assert_eq!(kodi_audio_codec(&AudioStream::default()), "");
    }

    #[test]
    fn images_and_names_per_consumer() {
        let s = MetadataConsumersSettings::default();
        let kodi = Consumer::Kodi(s.kodi);
        let names: Vec<String> = kodi
            .series_images(&series())
            .into_iter()
            .map(|i| i.relative_path)
            .collect();
        assert_eq!(names, vec!["poster.jpg", "fanart.png"]);
        let season = [MediaImage {
            cover_type: "poster".into(),
            remote_url: "https://x/s.jpg".into(),
        }];
        let folders = HashMap::from([(1, "Season 01".to_string())]);
        assert_eq!(
            kodi.season_images(&series(), 0, &season, &folders)[0].relative_path,
            "season-specials-poster.jpg"
        );
        assert_eq!(
            kodi.season_images(&series(), 2, &season, &folders)[0].relative_path,
            "season02-poster.jpg"
        );
        assert_eq!(
            kodi.episode_images(&file())[0].relative_path,
            "Season 01/Breaking Bad - S01E01 - Pilot-thumb.jpg"
        );
        let roksbox = Consumer::Roksbox(s.roksbox);
        assert_eq!(
            roksbox.series_images(&series())[0].relative_path,
            "Breaking Bad.jpg"
        );
        assert_eq!(
            roksbox.season_images(&series(), 1, &season, &folders)[0].relative_path,
            "Season 01/Season 01.jpg"
        );
        assert!(
            roksbox
                .season_images(&series(), 2, &season, &folders)
                .is_empty()
        );
        assert_eq!(
            roksbox.episode_images(&file())[0].relative_path,
            "Season 01/Breaking Bad - S01E01 - Pilot.jpg"
        );
        let wdtv = Consumer::Wdtv(s.wdtv);
        assert_eq!(wdtv.series_images(&series())[0].relative_path, "folder.jpg");
        assert_eq!(
            wdtv.season_images(&series(), 1, &season, &folders)[0].relative_path,
            "Season 01/folder.jpg"
        );
        assert_eq!(
            wdtv.episode_images(&file())[0].relative_path,
            "Season 01/Breaking Bad - S01E01 - Pilot.metathumb"
        );
        assert_eq!(
            kodi.file_name_after_move("Season 01/New.mkv", MetadataType::EpisodeImage, "x"),
            "Season 01/New-thumb.jpg"
        );
        assert_eq!(
            wdtv.file_name_after_move("Season 01/New.mkv", MetadataType::EpisodeMetadata, "x"),
            "Season 01/New.xml"
        );
    }

    #[test]
    fn roksbox_wdtv_and_plex_documents() {
        let s = MetadataConsumersSettings::default();
        let r = Consumer::Roksbox(s.roksbox)
            .episode_metadata(&series(), &file(), false)
            .unwrap();
        assert_eq!(
            r.relative_path,
            "Season 01/Breaking Bad - S01E01 - Pilot.xml"
        );
        assert_eq!(
            r.contents,
            "<video>\n  <title>Breaking Bad - 1x1 - Pilot</title>\n  <year>2008-01-20</year>\n  <genre>Crime / Drama</genre>\n  <actors></actors>\n  <description />\n  <length>47</length>\n  <mpaa>TV-MA</mpaa>\n</video>"
        );
        let w = Consumer::Wdtv(s.wdtv)
            .episode_metadata(&series(), &file(), false)
            .unwrap();
        assert!(
            w.contents.starts_with(
                "<details>\n  <id>7</id>\n  <title>Breaking Bad - 1x01 - Pilot</title>"
            )
        );
        assert!(w.contents.contains("<season_number>01</season_number>"));
        let plex = Consumer::Plex(PlexMetadataSettings {
            enable: true,
            series_plex_match_file: true,
            episode_mappings: true,
        });
        let mut two = file();
        two.episodes.push(EpisodeData {
            episode_number: 2,
            ..two.episodes[0].clone()
        });
        let m = plex
            .series_metadata(&series(), &[two], Reason::EpisodesImported)
            .unwrap();
        assert_eq!(m.relative_path, ".plexmatch");
        assert_eq!(
            m.contents,
            "Title: Breaking Bad\nYear: 2008\nTvdbId: 81189\nImdbId: tt0903747\nEpisode: S01E01-E02: Season 01/Breaking Bad - S01E01 - Pilot.mkv\n"
        );
    }

    #[test]
    fn existing_files_are_recognised() {
        // Sonarr's `FindMetadataFileFixture`s.
        let dir = tempfile::tempdir().unwrap();
        let s = MetadataConsumersSettings::default();
        let kodi = Consumer::Kodi(s.kodi);
        let p = |r: &str| dir.path().join(r);
        assert_eq!(
            kodi.find_metadata_file("tvshow.nfo", &p("tvshow.nfo")),
            Some((MetadataType::SeriesMetadata, None))
        );
        assert_eq!(
            kodi.find_metadata_file("fanart.jpg", &p("fanart.jpg")),
            Some((MetadataType::SeriesImage, None))
        );
        assert_eq!(
            kodi.find_metadata_file("season01-poster.jpg", &p("season01-poster.jpg")),
            Some((MetadataType::SeasonImage, Some(1)))
        );
        assert_eq!(
            kodi.find_metadata_file("season-specials-banner.png", &p("x")),
            Some((MetadataType::SeasonImage, Some(0)))
        );
        assert_eq!(
            kodi.find_metadata_file("Season 01/The.Series.S01E01-thumb.jpg", &p("x")),
            Some((MetadataType::EpisodeImage, None))
        );
        // An episode NFO only when it is Kodi's (scene NFOs are not).
        std::fs::write(
            p("The.Series.S01E01.nfo"),
            "<episodedetails><title>x</title></episodedetails>",
        )
        .unwrap();
        std::fs::write(p("The.Series.S01E02.nfo"), "Scene release information").unwrap();
        assert_eq!(
            kodi.find_metadata_file("The.Series.S01E01.nfo", &p("The.Series.S01E01.nfo")),
            Some((MetadataType::EpisodeMetadata, None))
        );
        assert_eq!(
            kodi.find_metadata_file("The.Series.S01E02.nfo", &p("The.Series.S01E02.nfo")),
            None
        );
        assert_eq!(
            kodi.find_metadata_file("The.Series.S01.nfo", &p("The.Series.S01.nfo")),
            None
        );

        let roksbox = Consumer::Roksbox(s.roksbox);
        assert_eq!(
            roksbox.find_metadata_file("Season 01/Season 01.jpg", &p("x")),
            Some((MetadataType::SeasonImage, Some(1)))
        );
        assert_eq!(
            roksbox.find_metadata_file("Specials/Specials.jpg", &p("x")),
            Some((MetadataType::SeasonImage, Some(0)))
        );
        assert_eq!(
            roksbox.find_metadata_file("Season 01/The.Series.S01E01.xml", &p("x")),
            Some((MetadataType::EpisodeMetadata, None))
        );
        assert_eq!(
            roksbox.find_metadata_file("Season 01/The.Series.S01E01.jpg", &p("x")),
            Some((MetadataType::EpisodeImage, None))
        );
        assert_eq!(
            roksbox.find_metadata_file("Season 01/The.Series.S01E01-thumb.jpg", &p("x")),
            None
        );
        assert_eq!(
            roksbox.find_metadata_file("metadata/The.Series.S01E01.jpg", &p("x")),
            None
        );

        let wdtv = Consumer::Wdtv(s.wdtv);
        assert_eq!(
            wdtv.find_metadata_file("folder.jpg", &p("x")),
            Some((MetadataType::SeriesImage, None))
        );
        assert_eq!(
            wdtv.find_metadata_file("Season 01/folder.jpg", &p("x")),
            Some((MetadataType::SeasonImage, Some(1)))
        );
        assert_eq!(
            wdtv.find_metadata_file("Season 01/The.Series.S01E01.metathumb", &p("x")),
            Some((MetadataType::EpisodeImage, None))
        );
        assert_eq!(
            Consumer::Plex(s.plex).find_metadata_file(".plexmatch", &p("x")),
            Some((MetadataType::SeriesMetadata, None))
        );
    }
}
