//! Notifications, ported from Sonarr's `Notifications`: an event (a grab, an import, a series
//! added or removed, a file deleted, a health check failing or recovering) goes to every
//! connection whose trigger for it is on and whose tags match the series
//! (`NotificationService`), each in its service's format.
//!
//! Implemented services: Webhook (Sonarr's JSON payloads), Notifiarr, Discord, Slack, Telegram,
//! Gotify, ntfy, Pushover, Pushbullet, Join, Prowl, Simplepush, Signal (signal-cli REST),
//! Apprise, Emby and Jellyfin (library refresh and notifications), Plex Media Server (library
//! refresh) and Kodi (library scan and notifications).

pub mod discord;
pub mod email;
pub mod extra;
pub mod media;
pub mod push;
pub mod script;
pub mod webhook;

use serde::Serialize;
use serde_json::{Map, Value};

/// A failed delivery.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NotifyError {
    #[error("{0}")]
    Settings(String),
    #[error("could not connect: {0}")]
    Connection(String),
    #[error("the service answered HTTP {status}{}", .message.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
    Http {
        status: u16,
        message: Option<String>,
    },
    #[error("{0}")]
    Service(String),
}

pub type NotifyResult<T = ()> = Result<T, NotifyError>;

/// Implementations Kitsunarr can deliver to (Sonarr's implementation names).
pub const SUPPORTED: &[&str] = &[
    "Webhook",
    "Notifiarr",
    "Discord",
    "Slack",
    "Telegram",
    "Gotify",
    "Ntfy",
    "Pushover",
    "PushBullet",
    "Join",
    "Prowl",
    "Simplepush",
    "Signal",
    "Apprise",
    "MediaBrowser",
    "Jellyfin",
    "PlexServer",
    "Xbmc",
    "CustomScript",
    "Email",
    "Mailgun",
    "SendGrid",
];

pub fn supported(implementation: &str) -> bool {
    SUPPORTED
        .iter()
        .any(|i| i.eq_ignore_ascii_case(implementation))
}

/// A series as notifications describe it (`WebhookSeries`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesRef {
    pub id: i64,
    pub title: String,
    pub title_slug: String,
    pub path: String,
    pub tvdb_id: i64,
    pub tv_maze_id: i64,
    pub tmdb_id: i64,
    pub imdb_id: Option<String>,
    pub mal_ids: Vec<i64>,
    pub ani_list_ids: Vec<i64>,
    /// `standard`, `daily` or `anime`.
    #[serde(rename = "type")]
    pub series_type: String,
    pub year: i32,
    pub genres: Vec<String>,
    pub images: Vec<ImageRef>,
    /// Tag labels.
    pub tags: Vec<String>,
    #[serde(skip)]
    pub tag_ids: Vec<i64>,
    pub original_language: Option<LanguageRef>,
    #[serde(skip)]
    pub overview: Option<String>,
    #[serde(skip)]
    pub rating: Option<f64>,
}

impl SeriesRef {
    pub fn poster(&self) -> Option<&str> {
        self.image("poster")
    }

    pub fn fanart(&self) -> Option<&str> {
        self.image("fanart")
    }

    fn image(&self, kind: &str) -> Option<&str> {
        self.images
            .iter()
            .find(|i| i.cover_type.eq_ignore_ascii_case(kind))
            .and_then(|i| i.remote_url.as_deref())
    }

    fn is_anime(&self) -> bool {
        self.series_type == "anime"
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    pub cover_type: String,
    pub url: Option<String>,
    pub remote_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageRef {
    pub id: i32,
    pub name: String,
}

/// An episode (`WebhookEpisode`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeRef {
    pub id: i64,
    pub episode_number: i32,
    pub season_number: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub air_date_utc: Option<String>,
    pub series_id: i64,
    pub tvdb_id: i64,
}

/// What was grabbed (`WebhookRelease`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRef {
    pub quality: String,
    pub quality_version: i32,
    pub release_group: Option<String>,
    pub release_title: String,
    pub indexer: String,
    pub size: i64,
    pub custom_format_score: i32,
    pub custom_formats: Vec<String>,
    pub languages: Vec<LanguageRef>,
    pub indexer_flags: Vec<String>,
}

/// The grab an import came from (`WebhookGrabbedRelease`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrabbedRef {
    pub release_title: Option<String>,
    pub indexer: Option<String>,
    pub size: Option<i64>,
    pub indexer_flags: Vec<String>,
    pub release_type: Option<String>,
}

/// A file in the library (`WebhookEpisodeFile`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    pub id: i64,
    pub relative_path: String,
    pub path: String,
    pub quality: String,
    pub quality_version: i32,
    pub release_group: Option<String>,
    pub scene_name: Option<String>,
    pub size: i64,
    pub date_added: String,
    pub languages: Vec<LanguageRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recycle_bin_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grab {
    pub series: SeriesRef,
    pub episodes: Vec<EpisodeRef>,
    pub release: ReleaseRef,
    pub download_client: String,
    pub download_client_type: String,
    pub download_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub series: SeriesRef,
    pub episodes: Vec<EpisodeRef>,
    pub file: FileRef,
    pub release: GrabbedRef,
    /// The files the import replaced; non-empty means an upgrade.
    pub deleted_files: Vec<FileRef>,
    pub download_client: Option<String>,
    pub download_client_type: Option<String>,
    pub download_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    /// `warning` or `error`.
    pub level: String,
    pub message: String,
    /// The check's name (`IndexerStatusCheck`, ...).
    pub source: String,
    pub wiki_url: Option<String>,
}

/// Something to tell the connections about.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Test,
    Grab(Box<Grab>),
    Download(Box<Download>),
    SeriesAdd(Box<SeriesRef>),
    SeriesDelete {
        series: Box<SeriesRef>,
        deleted_files: bool,
    },
    EpisodeFileDelete {
        series: Box<SeriesRef>,
        episodes: Vec<EpisodeRef>,
        file: FileRef,
        /// `MissingFromDisk`, `Manual`, `Upgrade`, `NoLinkedEpisodes`, `ManualOverride`.
        reason: String,
    },
    HealthIssue(Health),
    HealthRestored(Health),
    /// Episode files renamed (Sonarr's `OnRename`).
    Rename {
        series: Box<SeriesRef>,
        files: Vec<RenamedFile>,
    },
    /// A completed download that cannot be imported without the user (Sonarr's
    /// `OnManualInteractionRequired`).
    ManualInteractionRequired(Box<ManualInteraction>),
    /// Kitsunarr started with a new version (Sonarr's `OnApplicationUpdate`).
    ApplicationUpdate {
        previous_version: String,
        new_version: String,
    },
}

/// A download waiting for the user (`ManualInteractionRequiredMessage`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualInteraction {
    /// The series the download was grabbed for, when known.
    pub series: Option<SeriesRef>,
    pub episodes: Vec<EpisodeRef>,
    pub title: String,
    pub quality: String,
    pub quality_version: i32,
    pub size: i64,
    pub download_client: Option<String>,
    pub download_client_type: Option<String>,
    pub download_id: String,
    /// Why the import is blocked.
    pub messages: Vec<String>,
    pub release: GrabbedRef,
}

/// A renamed episode file (Sonarr's `RenamedEpisodeFile`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamedFile {
    pub id: i64,
    pub previous_relative_path: String,
    pub previous_path: String,
    pub relative_path: String,
    pub path: String,
}

impl Event {
    /// The series the event is about, for tag matching.
    pub fn series(&self) -> Option<&SeriesRef> {
        match self {
            Self::Grab(g) => Some(&g.series),
            Self::Download(d) => Some(&d.series),
            Self::SeriesAdd(s) => Some(s),
            Self::SeriesDelete { series, .. }
            | Self::EpisodeFileDelete { series, .. }
            | Self::Rename { series, .. } => Some(series),
            Self::ManualInteractionRequired(m) => m.series.as_ref(),
            _ => None,
        }
    }

    /// Sonarr's notification titles.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Test => "Test Notification",
            Self::Grab(_) => "Episode Grabbed",
            Self::Download(d) if !d.deleted_files.is_empty() => "Episode Upgraded",
            Self::Download(_) => "Episode Downloaded",
            Self::SeriesAdd(_) => "Series Added",
            Self::SeriesDelete { .. } => "Series Deleted",
            Self::EpisodeFileDelete { .. } => "Episode Deleted",
            Self::HealthIssue(_) => "Health Check Failure",
            Self::HealthRestored(_) => "Health Check Restored",
            Self::Rename { .. } => "Series Renamed",
            Self::ManualInteractionRequired(_) => "Manual Interaction Required",
            Self::ApplicationUpdate { .. } => "Application Updated",
        }
    }

    /// `NotificationService.GetMessage` and the other events' messages.
    pub fn message(&self) -> String {
        match self {
            Self::Test => "This is a test message from Kitsunarr".into(),
            Self::Grab(g) => episode_message(
                &g.series,
                &g.episodes,
                &quality_string(&g.series, &g.release.quality, g.release.quality_version),
            ),
            Self::Download(d) => episode_message(
                &d.series,
                &d.episodes,
                &quality_string(&d.series, &d.file.quality, d.file.quality_version),
            ),
            Self::SeriesAdd(s) => s.title.clone(),
            Self::SeriesDelete {
                series,
                deleted_files,
            } => {
                if *deleted_files {
                    format!("{} and all files were deleted", series.title)
                } else {
                    format!("{} was deleted", series.title)
                }
            }
            Self::EpisodeFileDelete {
                series,
                episodes,
                file,
                ..
            } => episode_message(
                series,
                episodes,
                &quality_string(series, &file.quality, file.quality_version),
            ),
            Self::HealthIssue(h) | Self::HealthRestored(h) => h.message.clone(),
            Self::Rename { series, files } => {
                format!("{}: {} files renamed", series.title, files.len())
            }
            Self::ManualInteractionRequired(m) => match &m.series {
                Some(series) => episode_message(
                    series,
                    &m.episodes,
                    &quality_string(series, &m.quality, m.quality_version),
                ),
                None => m.title.clone(),
            },
            Self::ApplicationUpdate {
                previous_version,
                new_version,
            } => format!("Kitsunarr updated from {previous_version} to {new_version}"),
        }
    }

    /// The trigger option that enables the event (Sonarr's `On*` switches).
    fn trigger(&self) -> &'static str {
        match self {
            Self::Test => "",
            Self::Grab(_) => "onGrab",
            Self::Download(_) => "onDownload",
            Self::SeriesAdd(_) => "onSeriesAdd",
            Self::SeriesDelete { .. } => "onSeriesDelete",
            Self::EpisodeFileDelete { .. } => "onEpisodeFileDelete",
            Self::HealthIssue(_) => "onHealthIssue",
            Self::HealthRestored(_) => "onHealthRestored",
            Self::Rename { .. } => "onRename",
            Self::ManualInteractionRequired(_) => "onManualInteractionRequired",
            Self::ApplicationUpdate { .. } => "onApplicationUpdate",
        }
    }
}

/// `GetQualityString`: "Proper" (anime: "v2") for revisions above 1.
pub fn quality_string(series: &SeriesRef, quality: &str, version: i32) -> String {
    if version > 1 {
        if series.is_anime() {
            format!("{quality} v{version}")
        } else {
            format!("{quality} Proper")
        }
    } else {
        quality.to_string()
    }
}

/// `GetMessage`: `Title - 1x01x02 - First + Second [Quality]` (daily: the air date).
pub fn episode_message(series: &SeriesRef, episodes: &[EpisodeRef], quality: &str) -> String {
    let Some(first) = episodes.first() else {
        return format!("{} - [{quality}]", series.title);
    };
    if series.series_type == "daily" {
        return format!(
            "{} - {} - {} [{quality}]",
            series.title,
            first.air_date.as_deref().unwrap_or_default(),
            first.title.as_deref().unwrap_or_default()
        );
    }
    let numbers: String = episodes
        .iter()
        .map(|e| format!("x{:02}", e.episode_number))
        .collect();
    let titles = episodes
        .iter()
        .map(|e| e.title.clone().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" + ");
    format!(
        "{} - {}{numbers} - {titles} [{quality}]",
        series.title, first.season_number
    )
}

/// A connection as stored (`NotificationDefinition`).
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub id: i64,
    pub name: String,
    pub implementation: String,
    /// The `on*` switches and `includeHealthWarnings`.
    pub triggers: Map<String, Value>,
    pub tags: Vec<i64>,
    pub settings: Map<String, Value>,
}

impl Connection {
    fn flag(&self, key: &str) -> bool {
        self.triggers
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// `NotificationService`'s filters: the event's switch (an upgrade also needs `onUpgrade`;
    /// health warnings need `includeHealthWarnings`), then the tags (`ShouldHandleSeries`).
    pub fn wants(&self, event: &Event) -> bool {
        if matches!(event, Event::Test) {
            return true;
        }
        if !self.flag(event.trigger()) {
            return false;
        }
        match event {
            Event::Download(d) if !d.deleted_files.is_empty() && !self.flag("onUpgrade") => {
                return false;
            }
            Event::HealthIssue(h) | Event::HealthRestored(h)
                if h.level == "warning" && !self.flag("includeHealthWarnings") =>
            {
                return false;
            }
            _ => {}
        }
        // Health events are not about a series and go out whatever the tags.
        if self.tags.is_empty() || matches!(event, Event::HealthIssue(_) | Event::HealthRestored(_))
        {
            return true;
        }
        event
            .series()
            .is_some_and(|s| s.tag_ids.iter().any(|t| self.tags.contains(t)))
    }
}

/// Instance details services include.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Instance {
    pub name: String,
    /// Settings › General's application URL, for links back.
    pub application_url: Option<String>,
}

/// Sends events to connections; shares one HTTP client.
#[derive(Clone, Default)]
pub struct Notifier {
    pub(crate) http: reqwest::Client,
}

impl Notifier {
    /// The HTTP client notifications use.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Delivers one event to one connection, in its service's format.
    pub async fn send(
        &self,
        c: &Connection,
        event: &Event,
        instance: &Instance,
    ) -> NotifyResult<()> {
        let s = &c.settings;
        match c.implementation.to_ascii_lowercase().as_str() {
            "webhook" => webhook::send(self, s, event, instance).await,
            "notifiarr" => webhook::notifiarr(self, s, event, instance).await,
            "discord" => discord::send(self, s, event, instance).await,
            "slack" => push::slack(self, s, event).await,
            "telegram" => push::telegram(self, s, event).await,
            "gotify" => push::gotify(self, s, event).await,
            "ntfy" => push::ntfy(self, s, event).await,
            "pushover" => push::pushover(self, s, event).await,
            "pushbullet" => push::pushbullet(self, s, event).await,
            "join" => push::join(self, s, event).await,
            "prowl" => push::prowl(self, s, event).await,
            "simplepush" => push::simplepush(self, s, event).await,
            "signal" => push::signal(self, s, event).await,
            "apprise" => push::apprise(self, s, event).await,
            "mediabrowser" | "jellyfin" => media::emby(self, s, event).await,
            "plexserver" => media::plex(self, s, event).await,
            "xbmc" => media::kodi(self, s, event).await,
            "customscript" => script::run(s, event, instance).await,
            "email" => email::smtp(s, event).await,
            "mailgun" => email::mailgun(self, s, event).await,
            "sendgrid" => email::sendgrid(self, s, event).await,
            "pushcut" => extra::pushcut(self, s, event).await,
            "synologyindexer" => extra::synology(s, event).await,
            "trakt" => extra::trakt(self, s, event).await,
            other => Err(NotifyError::Settings(format!(
                "{other} notifications are not supported yet"
            ))),
        }
    }

    /// Sends the event to every connection that wants it, logging failures (one failing
    /// connection never stops the others). Returns how many deliveries succeeded.
    pub async fn dispatch(
        &self,
        connections: &[Connection],
        event: &Event,
        instance: &Instance,
    ) -> usize {
        let mut sent = 0;
        for c in connections
            .iter()
            .filter(|c| supported(&c.implementation) && c.wants(event))
        {
            match self.send(c, event, instance).await {
                Ok(()) => sent += 1,
                Err(e) => {
                    tracing::warn!(connection = %c.name, event = event.title(), error = %e, "notification failed");
                }
            }
        }
        sent
    }

    /// POSTs JSON; maps transport and status errors.
    pub(crate) async fn post_json(
        &self,
        req: reqwest::RequestBuilder,
        body: &Value,
    ) -> NotifyResult<String> {
        let resp = req
            .json(body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(connection_error)?;
        check(resp).await
    }
}

pub(crate) fn connection_error(e: reqwest::Error) -> NotifyError {
    use std::error::Error as _;
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        msg = s.to_string();
        source = s.source();
    }
    if let Some(url) = e.url() {
        msg = msg.replace(url.as_str(), "<redacted>");
    }
    NotifyError::Connection(msg)
}

pub(crate) async fn check(resp: reqwest::Response) -> NotifyResult<String> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let message = Some(text.chars().take(200).collect::<String>()).filter(|m| !m.is_empty());
        return Err(NotifyError::Http {
            status: status.as_u16(),
            message,
        });
    }
    Ok(text)
}

pub(crate) fn text(s: &Map<String, Value>, key: &str) -> Option<String> {
    s.get(key)
        .and_then(|v| match v {
            Value::String(t) => Some(t.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
}

pub(crate) fn required(s: &Map<String, Value>, key: &str, label: &str) -> NotifyResult<String> {
    text(s, key).ok_or_else(|| NotifyError::Settings(format!("{label} is required")))
}

pub(crate) fn int(s: &Map<String, Value>, key: &str, default: i64) -> i64 {
    s.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok()))
        .unwrap_or(default)
}

pub(crate) fn flag(s: &Map<String, Value>, key: &str, default: bool) -> bool {
    s.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub(crate) fn strings(s: &Map<String, Value>, key: &str) -> Vec<String> {
    match s.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(|t| t.trim().to_string()))
            .filter(|t| !t.is_empty())
            .collect(),
        Some(Value::String(t)) => t
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// `{scheme}://{host}:{port}{urlBase}` for services configured by host and port.
pub(crate) fn base_url(s: &Map<String, Value>, default_port: i64) -> NotifyResult<String> {
    let host = required(s, "host", "The host")?;
    if host.contains('/') {
        return Err(NotifyError::Settings(
            "the host must be a host name or address, without a scheme or path".into(),
        ));
    }
    let scheme = if flag(s, "useSsl", false) {
        "https"
    } else {
        "http"
    };
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host
    };
    let base = text(s, "urlBase")
        .map(|b| format!("/{}", b.trim_matches('/')))
        .unwrap_or_default();
    Ok(format!(
        "{scheme}://{host}:{}{base}",
        int(s, "port", default_port)
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn series() -> SeriesRef {
        SeriesRef {
            id: 1,
            title: "Breaking Bad".into(),
            title_slug: "breaking-bad".into(),
            path: "/tv/Breaking Bad".into(),
            tvdb_id: 81189,
            series_type: "standard".into(),
            year: 2008,
            tag_ids: vec![3],
            tags: vec!["drama".into()],
            images: vec![ImageRef {
                cover_type: "poster".into(),
                url: None,
                remote_url: Some("https://img/poster.jpg".into()),
            }],
            ..Default::default()
        }
    }

    pub fn episode(n: i32, title: &str) -> EpisodeRef {
        EpisodeRef {
            id: n as i64,
            season_number: 1,
            episode_number: n,
            title: Some(title.into()),
            air_date: Some("2008-01-20".into()),
            series_id: 1,
            ..Default::default()
        }
    }

    pub fn grab() -> Event {
        Event::Grab(Box::new(Grab {
            series: series(),
            episodes: vec![episode(1, "Pilot"), episode(2, "Cat's in the Bag...")],
            release: ReleaseRef {
                quality: "HDTV-720p".into(),
                quality_version: 2,
                release_title: "Breaking.Bad.S01E01E02.PROPER.720p.HDTV.x264-GRP".into(),
                release_group: Some("GRP".into()),
                indexer: "Indexer".into(),
                size: 1_500_000_000,
                ..Default::default()
            },
            download_client: "qBittorrent".into(),
            download_client_type: "qBittorrent".into(),
            download_id: "ABC".into(),
        }))
    }

    #[test]
    fn messages_follow_sonarr() {
        assert_eq!(
            grab().message(),
            "Breaking Bad - 1x01x02 - Pilot + Cat's in the Bag... [HDTV-720p Proper]"
        );
        let mut anime = series();
        anime.series_type = "anime".into();
        assert_eq!(quality_string(&anime, "HDTV-720p", 2), "HDTV-720p v2");
        let mut daily = series();
        daily.series_type = "daily".into();
        assert_eq!(
            episode_message(&daily, &[episode(1, "Show")], "WEBDL-1080p"),
            "Breaking Bad - 2008-01-20 - Show [WEBDL-1080p]"
        );
        assert_eq!(episode_message(&series(), &[], "x"), "Breaking Bad - [x]");
    }

    #[test]
    fn triggers_tags_upgrades_and_health_warnings() {
        let conn = |triggers: Value, tags: Vec<i64>| Connection {
            id: 1,
            name: "c".into(),
            implementation: "Webhook".into(),
            triggers: triggers.as_object().unwrap().clone(),
            tags,
            settings: Map::new(),
        };
        let g = grab();
        assert!(conn(json!({"onGrab": true}), vec![]).wants(&g));
        assert!(!conn(json!({"onGrab": false}), vec![]).wants(&g));
        assert!(conn(json!({"onGrab": true}), vec![3]).wants(&g));
        assert!(!conn(json!({"onGrab": true}), vec![4]).wants(&g));
        let upgrade = Event::Download(Box::new(Download {
            series: series(),
            episodes: vec![],
            file: FileRef::default(),
            release: GrabbedRef::default(),
            deleted_files: vec![FileRef::default()],
            download_client: None,
            download_client_type: None,
            download_id: None,
        }));
        assert!(!conn(json!({"onDownload": true}), vec![]).wants(&upgrade));
        assert!(conn(json!({"onDownload": true, "onUpgrade": true}), vec![]).wants(&upgrade));
        let warning = Event::HealthIssue(Health {
            level: "warning".into(),
            message: "m".into(),
            source: "s".into(),
            wiki_url: None,
        });
        assert!(!conn(json!({"onHealthIssue": true}), vec![]).wants(&warning));
        assert!(
            conn(
                json!({"onHealthIssue": true, "includeHealthWarnings": true}),
                vec![]
            )
            .wants(&warning)
        );
        // Health events have no series: tags do not filter them (upstream).
        assert!(
            conn(
                json!({"onHealthIssue": true, "includeHealthWarnings": true}),
                vec![3]
            )
            .wants(&warning)
        );
        assert!(conn(json!({}), vec![9]).wants(&Event::Test));
    }

    #[test]
    fn base_urls() {
        let s = json!({"host": "emby", "port": 8096, "useSsl": true, "urlBase": "/emby/"});
        assert_eq!(
            base_url(s.as_object().unwrap(), 1).unwrap(),
            "https://emby:8096/emby"
        );
        let s = json!({"host": "http://x"});
        assert!(base_url(s.as_object().unwrap(), 1).is_err());
    }
}
