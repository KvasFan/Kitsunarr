//! Newznab and Torznab feed parsing (`RssParser.cs`, `NewznabRssParser.cs`, `TorznabRssParser.cs`,
//! `TorrentRssParser.cs`); the other RSS indexers' parsers live in [`crate::torrent_rss`].

use chrono::{DateTime, NaiveDateTime, Utc};
use kitsunarr_domain::{DownloadProtocol, Language};
use serde::Serialize;

use crate::error::IndexerError;
use crate::xml::{self, Element};

pub const NEWZNAB_NS: &str = "http://www.newznab.com/DTD/2010/feeds/attributes/";
pub const TORZNAB_NS: &str = "http://torznab.com/schemas/2015/feed";

const NZB_MIME: &str = "application/x-nzb";
const TORRENT_MIMES: &[&str] = &[
    "application/x-bittorrent",
    "application/x-bittorrent;x-scheme-handler/magnet",
];

/// `IndexerFlags` (bit values as upstream).
pub mod flags {
    pub const FREELEECH: u32 = 1;
    pub const HALFLEECH: u32 = 2;
    pub const DOUBLE_UPLOAD: u32 = 4;
    pub const INTERNAL: u32 = 8;
    pub const SCENE: u32 = 16;
    pub const FREELEECH75: u32 = 32;
    pub const FREELEECH25: u32 = 64;
    pub const NUKED: u32 = 128;
    pub const SUBTITLES: u32 = 256;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Newznab,
    Torznab,
    /// Sonarr's `Nyaa`: a torrent RSS feed with `nyaa:` elements, searched by title.
    Nyaa,
    /// Sonarr's `TorrentRssIndexer`: any torrent RSS feed, its layout detected from the feed.
    TorrentRss,
    /// Sonarr's `IPTorrents`: the tracker's RSS feed (RSS only).
    IpTorrents,
    /// Sonarr's `Torrentleech`: the tracker's RSS feed (RSS only).
    Torrentleech,
    /// Sonarr's `Fanzub`: a usenet anime RSS feed, searched by title.
    Fanzub,
    /// Sonarr's `FileList`: the tracker's JSON API.
    FileList,
    /// Sonarr's `HDBits`: the tracker's JSON API (POST).
    HdBits,
    /// Sonarr's `BroadcastheNet`: the tracker's JSON-RPC API.
    BroadcastheNet,
}

impl Dialect {
    /// The dialect of a provider implementation (Sonarr's class names).
    pub fn from_implementation(implementation: &str) -> Option<Self> {
        Some(match implementation {
            "Newznab" => Self::Newznab,
            "Torznab" => Self::Torznab,
            "Nyaa" => Self::Nyaa,
            "TorrentRssIndexer" => Self::TorrentRss,
            "IPTorrents" => Self::IpTorrents,
            "Torrentleech" => Self::Torrentleech,
            "Fanzub" => Self::Fanzub,
            "FileList" => Self::FileList,
            "HDBits" => Self::HdBits,
            "BroadcastheNet" => Self::BroadcastheNet,
            _ => return None,
        })
    }

    pub fn protocol(self) -> DownloadProtocol {
        match self {
            Self::Newznab | Self::Fanzub => DownloadProtocol::Usenet,
            _ => DownloadProtocol::Torrent,
        }
    }

    /// Torrent indexers (minimum seeders, seed criteria, complete-series searches).
    pub fn is_torrent(self) -> bool {
        self.protocol() == DownloadProtocol::Torrent
    }

    /// `SupportsSearch`: the RSS-only indexers generate no search requests.
    pub fn supports_search(self) -> bool {
        !matches!(
            self,
            Self::TorrentRss | Self::IpTorrents | Self::Torrentleech
        )
    }

    /// Newznab and Torznab read a capabilities document; the others search their own way.
    pub fn has_capabilities(self) -> bool {
        matches!(self, Self::Newznab | Self::Torznab)
    }

    /// Indexers answering JSON instead of an RSS feed.
    pub fn is_json(self) -> bool {
        matches!(self, Self::FileList | Self::HdBits | Self::BroadcastheNet)
    }
}

/// One release from an indexer feed (`ReleaseInfo` / `TorrentInfo`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub guid: String,
    pub title: String,
    pub size: i64,
    pub download_url: Option<String>,
    pub info_url: Option<String>,
    pub comment_url: Option<String>,
    pub publish_date: DateTime<Utc>,
    pub protocol: DownloadProtocol,
    pub tvdb_id: i64,
    pub tv_rage_id: i64,
    pub imdb_id: Option<String>,
    pub languages: Vec<&'static Language>,
    pub indexer_flags: u32,
    pub info_hash: Option<String>,
    pub magnet_url: Option<String>,
    pub seeders: Option<i64>,
    pub peers: Option<i64>,
}

/// `<error code=… description=…/>` anywhere in the document (`CheckError`).
pub(crate) fn check_error(
    doc: &Element,
    newznab: bool,
    has_api_key: bool,
) -> Result<(), IndexerError> {
    let mut errors = Vec::new();
    if doc.name == "error" {
        errors.push(doc);
    }
    doc.descendants("error", &mut errors);
    let Some(error) = errors.first() else {
        return Ok(());
    };
    let code: i32 = error
        .attr("code")
        .and_then(|c| c.trim().parse().ok())
        .ok_or_else(|| IndexerError::Unexpected("error element without a code".into()))?;
    let description = error.attr("description").unwrap_or_default().to_string();
    if (100..=199).contains(&code) {
        return Err(IndexerError::ApiKey(if newznab {
            description
        } else {
            "Invalid API key".into()
        }));
    }
    if !has_api_key
        && (description == "Missing parameter" || (newznab && description.contains("apikey")))
    {
        return Err(IndexerError::ApiKey("Indexer requires an API key".into()));
    }
    if description == "Request limit reached" {
        return Err(IndexerError::RequestLimit {
            retry_after_secs: None,
        });
    }
    Err(IndexerError::Indexer(if newznab {
        description
    } else {
        format!("Torznab error detected: {description}")
    }))
}

/// `XElementExtensions.ParseDate`: RFC 822 dates and the other formats .NET accepts,
/// assumed UTC when no zone is given.
pub fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    let v = value.trim();
    if let Ok(d) = DateTime::parse_from_rfc2822(v) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(v) {
        return Some(d.with_timezone(&Utc));
    }
    for fmt in [
        "%a, %d %b %Y %H:%M:%S %z",
        "%d %b %Y %H:%M:%S %z",
        "%Y-%m-%d %H:%M:%S %z",
    ] {
        if let Ok(d) = DateTime::parse_from_str(v, fmt) {
            return Some(d.with_timezone(&Utc));
        }
    }
    // Without a usable zone (e.g. "EST" or none): drop a trailing zone name, assume UTC.
    let stripped = v
        .rsplit_once(' ')
        .filter(|(_, zone)| {
            zone.chars().all(|c| c.is_ascii_alphabetic()) && (2..=5).contains(&zone.len())
        })
        .map_or(v, |(rest, _)| rest);
    for fmt in [
        "%a, %d %b %Y %H:%M:%S",
        "%d %b %Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
    ] {
        if let Ok(d) = NaiveDateTime::parse_from_str(stripped, fmt) {
            return Some(d.and_utc());
        }
    }
    None
}

struct Enclosure {
    url: Option<String>,
    mime: Option<String>,
    length: i64,
}

fn enclosures(item: &Element) -> Vec<Enclosure> {
    item.children_named("enclosure")
        .map(|e| Enclosure {
            url: e.attr("url").map(str::to_string),
            mime: e.attr("type").map(str::to_string),
            length: e
                .attr("length")
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(0),
        })
        .collect()
}

/// `GetEnclosure` with preferred MIME types (no fallback when none matches).
fn preferred<'a>(list: &'a [Enclosure], mimes: &[&str]) -> Option<&'a Enclosure> {
    mimes
        .iter()
        .find_map(|m| list.iter().find(|e| e.mime.as_deref() == Some(*m)))
}

fn is_absolute(url: &str) -> bool {
    url::Url::parse(url).is_ok()
}

/// `ParseUrl`: resolves relative URLs against the request URL.
fn resolve_url(base: &url::Url, value: Option<&str>) -> Option<String> {
    let v = value?.trim();
    if v.is_empty() {
        return None;
    }
    match base.join(v) {
        Ok(u) => Some(u.to_string()),
        Err(_) => Some(v.to_string()),
    }
}

struct Attrs<'a> {
    list: Vec<&'a Element>,
}

impl<'a> Attrs<'a> {
    fn new(item: &'a Element, ns: &'a str) -> Self {
        Self {
            list: item.children_ns(ns, "attr").collect(),
        }
    }

    /// First attribute with this name (case-insensitive), or empty.
    fn get(&self, key: &str) -> &'a str {
        self.list
            .iter()
            .find(|e| e.attr("name").is_some_and(|n| n.eq_ignore_ascii_case(key)))
            .and_then(|e| e.attr("value"))
            .unwrap_or("")
    }

    fn all(&self, key: &str) -> Vec<&'a str> {
        self.list
            .iter()
            .filter(|e| e.attr("name").is_some_and(|n| n.eq_ignore_ascii_case(key)))
            .filter_map(|e| e.attr("value"))
            .collect()
    }
}

fn int_or_zero(v: &str) -> i64 {
    v.trim().parse().unwrap_or(0)
}

fn imdb(v: &str) -> Option<String> {
    v.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| format!("tt{n:07}"))
}

/// `IsoLanguages.FindByName`: catalogue languages with an ISO entry, matched by name.
fn language_by_name(name: &str) -> Option<&'static Language> {
    Language::by_name(name.trim()).filter(|l| l.id > 0)
}

/// Parses a feed. `request_url` resolves relative links and tells whether an API key was sent.
pub fn parse_feed(
    body: &str,
    dialect: Dialect,
    request_url: &str,
) -> Result<Vec<Release>, IndexerError> {
    match dialect {
        Dialect::TorrentRss => {
            // Without a cached layout the feed's own layout is detected first.
            let settings = crate::torrent_rss::detect(body, request_url, false)?;
            return crate::torrent_rss::parser_for(&settings).parse(body, request_url);
        }
        Dialect::IpTorrents => {
            return crate::torrent_rss::RssParser::iptorrents().parse(body, request_url);
        }
        Dialect::Torrentleech => {
            return crate::torrent_rss::RssParser::torrentleech().parse(body, request_url);
        }
        Dialect::Fanzub => return crate::torrent_rss::RssParser::fanzub().parse(body, request_url),
        Dialect::FileList | Dialect::HdBits | Dialect::BroadcastheNet => {
            return Err(IndexerError::Unexpected(format!(
                "{dialect:?} answers JSON, not an RSS feed"
            )));
        }
        Dialect::Newznab | Dialect::Torznab | Dialect::Nyaa => {}
    }
    let base = url::Url::parse(request_url).map_err(|e| IndexerError::Unexpected(e.to_string()))?;
    let doc = xml::parse(body).map_err(IndexerError::InvalidXml)?;
    if dialect == Dialect::Nyaa {
        return parse_nyaa(&doc, &base);
    }
    check_error(
        &doc,
        dialect == Dialect::Newznab,
        request_url.contains("apikey="),
    )?;
    let items: Vec<&Element> = doc
        .child("channel")
        .map(|c| c.children_named("item").collect())
        .unwrap_or_default();

    // A Newznab feed with only torrent enclosures (or the reverse) is the wrong indexer type.
    let mut types: Vec<String> = items
        .iter()
        .flat_map(|i| enclosures(i).into_iter().filter_map(|e| e.mime))
        .collect();
    types.sort();
    types.dedup();
    let (own, other): (&[&str], &[&str]) = match dialect {
        Dialect::Newznab => (&[NZB_MIME], TORRENT_MIMES),
        _ => (TORRENT_MIMES, &[NZB_MIME]),
    };
    if !types.is_empty()
        && !types.iter().any(|t| own.contains(&t.as_str()))
        && types.iter().any(|t| other.contains(&t.as_str()))
    {
        tracing::warn!(
            ?dialect,
            "feed contains only {} enclosures; is this the right indexer type?",
            if dialect == Dialect::Newznab {
                "torrent"
            } else {
                "NZB"
            }
        );
        return Ok(Vec::new());
    }

    let ns = match dialect {
        Dialect::Newznab => NEWZNAB_NS,
        _ => TORZNAB_NS,
    };
    let mut releases = Vec::with_capacity(items.len());
    for item in items {
        match parse_item(item, dialect, ns, &base) {
            Ok(r) => releases.push(r),
            // A missing pubDate makes the whole feed unsupported (upstream rethrows it).
            Err(ItemError::Feed(e)) => return Err(e),
            // Any other problem skips the item, as upstream's per-item catch does.
            Err(ItemError::Skip(reason)) => {
                tracing::warn!(
                    title = item.child_text("title").unwrap_or_default(),
                    reason,
                    "skipping feed item"
                );
            }
        }
    }
    Ok(releases)
}

/// Sonarr's `TorrentRssParser` as `Nyaa` configures it: the link is the torrent, the guid the
/// info page, and `nyaa:seeders`, `nyaa:leechers` (peers = both), `nyaa:infoHash` and
/// `nyaa:size` ("1.4 GiB") describe the torrent.
fn parse_nyaa(doc: &Element, base: &url::Url) -> Result<Vec<Release>, IndexerError> {
    let items: Vec<&Element> = doc
        .child("channel")
        .map(|c| c.children_named("item").collect())
        .unwrap_or_default();
    let any = |item: &Element, name: &str| {
        item.children
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.text.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(date) = item.child_text("pubDate").and_then(parse_date) else {
            continue;
        };
        let Some(link) = resolve_url(base, item.child_text("link")) else {
            continue;
        };
        let guid = item
            .child_text("guid")
            .map(str::to_string)
            .unwrap_or_else(|| link.clone());
        let seeders = any(item, "seeders").and_then(|v| v.parse::<i64>().ok());
        let leechers = any(item, "leechers").and_then(|v| v.parse::<i64>().ok());
        let info_hash = any(item, "infoHash").map(|h| h.to_ascii_lowercase());
        out.push(Release {
            info_url: resolve_url(base, Some(guid.as_str())),
            guid,
            title: xml::html_decode(item.child_text("title").unwrap_or("Unknown")),
            size: any(item, "size").as_deref().map_or(0, parse_size),
            // Like upstream's `TorrentRssParser` for Nyaa: the .torrent link, no magnet.
            magnet_url: None,
            download_url: Some(link),
            comment_url: None,
            publish_date: date,
            protocol: DownloadProtocol::Torrent,
            tvdb_id: 0,
            tv_rage_id: 0,
            imdb_id: None,
            languages: Vec::new(),
            indexer_flags: 0,
            info_hash,
            seeders,
            peers: match (seeders, leechers) {
                (Some(s), Some(l)) => Some(s + l),
                (s, _) => s,
            },
        });
    }
    Ok(out)
}

/// `ParseSize`: "1.4 GiB", "700 MiB", "512 KB", or plain bytes.
pub(crate) fn parse_size(text: &str) -> i64 {
    let t = text.trim().replace(',', "");
    let (number, unit) = match t.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (t[..i].trim(), t[i..].trim().to_ascii_lowercase()),
        None => (t.as_str(), String::new()),
    };
    let Ok(n) = number.parse::<f64>() else {
        return 0;
    };
    let factor: f64 = match unit.as_str() {
        "kib" | "kb" => 1024.0,
        "mib" | "mb" => 1024.0 * 1024.0,
        "gib" | "gb" => 1024.0 * 1024.0 * 1024.0,
        "tib" | "tb" => 1024.0_f64.powi(4),
        _ => 1.0,
    };
    // `Convert.ToInt64`: rounded, ties to even.
    (n * factor).round_ties_even() as i64
}

enum ItemError {
    Feed(IndexerError),
    Skip(String),
}

fn parse_item(
    item: &Element,
    dialect: Dialect,
    ns: &str,
    base: &url::Url,
) -> Result<Release, ItemError> {
    {
        let attrs = Attrs::new(item, ns);
        let encl = enclosures(item);
        let date_text = match dialect {
            Dialect::Newznab => Some(attrs.get("usenetdate")).filter(|d| !d.trim().is_empty()),
            _ => None,
        }
        .or(item.child_text("pubDate"))
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| {
            ItemError::Feed(IndexerError::Unexpected(
                "each item in the feed must have a pubDate element with a valid publish date"
                    .into(),
            ))
        })?;
        let publish_date = parse_date(date_text)
            .ok_or_else(|| ItemError::Skip(format!("unparseable date {date_text:?}")))?;

        let download_url = match dialect {
            Dialect::Newznab => resolve_url(
                base,
                preferred(&encl, &[NZB_MIME]).and_then(|e| e.url.as_deref()),
            ),
            _ => {
                let url = resolve_url(
                    base,
                    preferred(&encl, TORRENT_MIMES).and_then(|e| e.url.as_deref()),
                );
                match url {
                    Some(u) if is_absolute(&u) => Some(u),
                    _ => resolve_url(base, encl.first().and_then(|e| e.url.as_deref())),
                }
            }
        };
        let size_attr = attrs.get("size").trim().parse::<i64>().ok();
        let size = size_attr.unwrap_or_else(|| {
            let mimes: &[&str] = match dialect {
                Dialect::Newznab => &[NZB_MIME],
                _ => TORRENT_MIMES,
            };
            preferred(&encl, mimes).map_or(0, |e| e.length)
        });
        let comments = item.child_text("comments").unwrap_or_default();
        let languages: Vec<&'static Language> = {
            let mut values: Vec<String> = attrs
                .all("language")
                .into_iter()
                .map(str::to_string)
                .collect();
            if values.is_empty() {
                values = item
                    .children_named("language")
                    .map(|e| e.text.clone())
                    .collect();
            }
            values
                .iter()
                .flat_map(|v| match dialect {
                    Dialect::Newznab => v
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>(),
                    _ => vec![v.as_str()],
                })
                .filter_map(language_by_name)
                .collect()
        };

        let mut release = Release {
            guid: item
                .child_text("guid")
                .map(str::to_string)
                .unwrap_or_else(random_guid),
            title: xml::html_decode(item.child_text("title").unwrap_or("Unknown")),
            size,
            download_url,
            info_url: resolve_url(
                base,
                Some(comments.strip_suffix("#comments").unwrap_or(comments)),
            ),
            comment_url: resolve_url(base, Some(comments)),
            publish_date,
            protocol: dialect.protocol(),
            tvdb_id: int_or_zero(attrs.get("tvdbid")),
            tv_rage_id: int_or_zero(attrs.get("rageid")),
            imdb_id: imdb(attrs.get("imdb")),
            languages,
            indexer_flags: 0,
            info_hash: None,
            magnet_url: None,
            seeders: None,
            peers: None,
        };
        match dialect {
            Dialect::Newznab => {
                if attrs.get("prematch") == "1" || attrs.get("haspretime") == "1" {
                    release.indexer_flags |= flags::SCENE;
                }
                if attrs.get("nuked") == "1" {
                    release.indexer_flags |= flags::NUKED;
                }
                if !attrs.get("subs").trim().is_empty() {
                    release.indexer_flags |= flags::SUBTITLES;
                }
            }
            _ => {
                let factor = |k: &str| attrs.get(k).trim().parse::<f32>().unwrap_or(1.0);
                let (down, up) = (factor("downloadvolumefactor"), factor("uploadvolumefactor"));
                for (value, flag) in [
                    (0.5, flags::HALFLEECH),
                    (0.75, flags::FREELEECH25),
                    (0.25, flags::FREELEECH75),
                    (0.0, flags::FREELEECH),
                ] {
                    if down == value {
                        release.indexer_flags |= flag;
                    }
                }
                if up == 2.0 {
                    release.indexer_flags |= flags::DOUBLE_UPLOAD;
                }
                let tags = attrs.all("tag");
                if tags.iter().any(|t| t.eq_ignore_ascii_case("internal")) {
                    release.indexer_flags |= flags::INTERNAL;
                }
                if tags.iter().any(|t| t.eq_ignore_ascii_case("scene")) {
                    release.indexer_flags |= flags::SCENE;
                }
                release.info_hash = Some(attrs.get("infohash"))
                    .filter(|h| !h.is_empty())
                    .map(str::to_string);
                release.magnet_url = Some(attrs.get("magneturl"))
                    .filter(|m| !m.is_empty())
                    .map(str::to_string);
                // `int.Parse`: a present but invalid number skips the item.
                let int = |k: &str| -> Result<Option<i64>, ItemError> {
                    let v = attrs.get(k).trim();
                    if v.is_empty() {
                        return Ok(None);
                    }
                    v.parse()
                        .map(Some)
                        .map_err(|_| ItemError::Skip(format!("invalid {k} {v:?}")))
                };
                release.seeders = int("seeders")?;
                release.peers = match int("peers")? {
                    Some(p) => Some(p),
                    None => match (int("seeders")?, int("leechers")?) {
                        (Some(s), Some(l)) => Some(s + l),
                        _ => None,
                    },
                };
            }
        }
        Ok(release)
    }
}

/// `Guid.NewGuid()` stand-in for items without a guid.
pub(crate) fn random_guid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64;
    format!("kitsunarr-{t:x}-{n:x}")
}

/// `IsValidRelease`: a title and a download URL.
pub fn is_valid(release: &Release) -> bool {
    !release.title.trim().is_empty()
        && release
            .download_url
            .as_deref()
            .is_some_and(|u| !u.trim().is_empty())
}
