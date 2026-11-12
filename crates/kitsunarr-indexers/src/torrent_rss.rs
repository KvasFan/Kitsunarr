//! Sonarr's generic RSS parsers (`RssParser.cs`, `TorrentRssParser.cs`,
//! `EzrssTorrentRssParser.cs`), the Torrent RSS feed indexer's layout detection and cached
//! parser factory (`TorrentRss/`), and the RSS-only IPTorrents and TorrentLeech indexers, whose
//! requests and settings rules live here too.
//!
//! The parsers are configured as upstream configures them: Fanzub reads a plain usenet RSS feed
//! (`RssParser { UseEnclosureUrl, UseEnclosureLength }`), IPTorrents parses sizes in the
//! description, TorrentLeech reads seeders there and uses the guid as the info page, and a
//! Torrent RSS feed gets the layout its first response was detected to have.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use kitsunarr_domain::DownloadProtocol;
use kitsunarr_parser::netregex::NetRegex;

use crate::error::IndexerError;
use crate::feed::{Dialect, Release, parse_date, random_guid};
use crate::request::{ACCEPT_RSS, Chain, Pageable, Request, Settings};
use crate::xml::{self, Element};

/// `RssParser.TorrentEnclosureMimeTypes`.
const TORRENT_MIMES: &[&str] = &[
    "application/x-bittorrent",
    "application/x-bittorrent;x-scheme-handler/magnet",
];

/// Releases smaller than this are taken for a feed without real sizes (`ValidSizeThreshold`).
pub const VALID_SIZE_THRESHOLD: i64 = 2 * 1024 * 1024;

/// How long a detected feed layout is kept (`TorrentRssParserFactory`: 7 days).
const PARSER_SETTINGS_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

const EZRSS_NS: &str = "http://xmlns.ezrss.it/0.1/";

/// `RssParser` / `TorrentRssParser` with their switches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RssParser {
    /// A `TorrentRssParser` (torrent releases, hashes and peers); otherwise a usenet `RssParser`.
    pub torrent: bool,
    /// `UseGuidInfoUrl`: the guid is the info page.
    pub use_guid_info_url: bool,
    /// `UseEnclosureUrl`: the enclosure is the download link (otherwise `<link>`).
    pub use_enclosure_url: bool,
    /// `UseEnclosureLength`: the enclosure's length is the size.
    pub use_enclosure_length: bool,
    /// `ParseSizeInDescription`: "Size: 1.3 GB" in the description.
    pub parse_size_in_description: bool,
    /// `PreferredEnclosureMimeTypes` (enforced: no other enclosure is used).
    pub preferred_enclosure_mime_types: Option<&'static [&'static str]>,
    /// `CalculatePeersAsSum`: peers = leechers + seeders.
    pub calculate_peers_as_sum: bool,
    /// `InfoHashElementName`.
    pub info_hash_element: Option<String>,
    /// `ParseSeedersInDescription`: "Seeders: 6", "3 peers", … in the description.
    pub parse_seeders_in_description: bool,
    /// `PeersElementName`.
    pub peers_element: Option<String>,
    /// `SeedsElementName`.
    pub seeds_element: Option<String>,
    /// `SizeElementName`, used when the size is otherwise 0.
    pub size_element: Option<String>,
    /// `MagnetElementName`.
    pub magnet_element: Option<String>,
    /// `EzrssTorrentRssParser.PreProcess`: a lone "No items exist" item is an error.
    pub ezrss: bool,
}

/// An item that fails to parse is skipped; a feed-level problem fails the whole feed.
enum ItemError {
    Feed(IndexerError),
    Skip(String),
}

impl From<String> for ItemError {
    fn from(s: String) -> Self {
        Self::Skip(s)
    }
}

struct Enclosure {
    url: Option<String>,
    mime: Option<String>,
    length: i64,
}

/// `FindDecendants`: descendants by local name, case-insensitively, in any namespace.
fn find_descendants<'a>(item: &'a Element, name: &str) -> Vec<&'a Element> {
    fn walk<'a>(e: &'a Element, name: &str, out: &mut Vec<&'a Element>) {
        for c in &e.children {
            if c.name.eq_ignore_ascii_case(name) {
                out.push(c);
            }
            walk(c, name, out);
        }
    }
    let mut out = Vec::new();
    walk(item, name, &mut out);
    out
}

/// `XElement.Value`: the text of the element and its descendants.
fn value(e: &Element) -> String {
    let mut out = e.text.clone();
    for c in &e.children {
        out.push_str(&value(c));
    }
    out
}

/// `SingleOrDefault` over the descendants with this name.
fn single_descendant<'a>(item: &'a Element, name: &str) -> Result<Option<&'a Element>, String> {
    let found = find_descendants(item, name);
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found[0])),
        _ => Err(format!("more than one {name} element")),
    }
}

/// `int.Parse`.
fn parse_int(v: &str) -> Result<i64, String> {
    v.trim()
        .parse::<i32>()
        .map(i64::from)
        .map_err(|_| format!("invalid number {v:?}"))
}

impl RssParser {
    /// A plain `RssParser` (usenet).
    pub fn usenet() -> Self {
        Self::default()
    }

    /// `new TorrentRssParser()`: torrent enclosures preferred.
    pub fn torrent() -> Self {
        Self {
            torrent: true,
            preferred_enclosure_mime_types: Some(TORRENT_MIMES),
            ..Self::default()
        }
    }

    /// `EzrssTorrentRssParser`.
    pub fn ezrss() -> Self {
        Self {
            use_guid_info_url: true,
            use_enclosure_length: false,
            use_enclosure_url: true,
            seeds_element: Some("seeds".into()),
            info_hash_element: Some("infoHash".into()),
            size_element: Some("contentLength".into()),
            magnet_element: Some("magnetURI".into()),
            ezrss: true,
            ..Self::torrent()
        }
    }

    /// `IPTorrents.GetParser`.
    pub fn iptorrents() -> Self {
        Self {
            parse_size_in_description: true,
            ..Self::torrent()
        }
    }

    /// `Torrentleech.GetParser`.
    pub fn torrentleech() -> Self {
        Self {
            use_guid_info_url: true,
            parse_seeders_in_description: true,
            ..Self::torrent()
        }
    }

    /// `Fanzub.GetParser`.
    pub fn fanzub() -> Self {
        Self {
            use_enclosure_url: true,
            use_enclosure_length: true,
            ..Self::usenet()
        }
    }

    /// `ParseResponse`: the feed's items, skipping those that fail to parse. A missing
    /// `pubDate` fails the whole feed (`UnsupportedFeedException`).
    pub fn parse(&self, body: &str, request_url: &str) -> Result<Vec<Release>, IndexerError> {
        let base =
            url::Url::parse(request_url).map_err(|e| IndexerError::Unexpected(e.to_string()))?;
        let doc = xml::parse(body).map_err(IndexerError::InvalidXml)?;
        let items = items(&doc);
        if self.ezrss
            && items.len() == 1
            && title(items[0]).as_str() == "No items exist - Try again later"
        {
            return Err(IndexerError::Indexer("No results were found".into()));
        }
        let mut releases = Vec::with_capacity(items.len());
        for item in items {
            match self.process_item(item, &base) {
                Ok(r) => releases.push(r),
                Err(ItemError::Feed(e)) => return Err(e),
                Err(ItemError::Skip(reason)) => {
                    tracing::warn!(title = title(item), reason, "skipping feed item")
                }
            }
        }
        Ok(releases)
    }

    fn process_item(&self, item: &Element, base: &url::Url) -> Result<Release, ItemError> {
        let guid = item.child("guid").map(value).unwrap_or_else(random_guid);
        let title = title(item);
        let date_text = item.child("pubDate").map(value).unwrap_or_default();
        if date_text.trim().is_empty() {
            return Err(ItemError::Feed(IndexerError::Indexer(
                "Each item in the RSS feed must have a pubDate element with a valid publish date."
                    .into(),
            )));
        }
        let publish_date =
            parse_date(&date_text).ok_or_else(|| format!("unparseable date {date_text:?}"))?;
        let download_url = self.download_url(item, base)?;
        let info_url = if self.use_guid_info_url {
            parse_url(base, item.child("guid").map(value).as_deref())
        } else {
            None
        };
        let comment_url = parse_url(base, item.child("comments").map(value).as_deref());
        let size = self
            .size(item)
            .map_err(|_| format!("Unable to parse size from: {title}"))?;
        let mut release = Release {
            guid,
            title,
            size,
            download_url,
            info_url,
            comment_url,
            publish_date,
            protocol: if self.torrent {
                DownloadProtocol::Torrent
            } else {
                DownloadProtocol::Usenet
            },
            tvdb_id: 0,
            tv_rage_id: 0,
            imdb_id: None,
            languages: Vec::new(),
            indexer_flags: 0,
            info_hash: None,
            magnet_url: None,
            seeders: None,
            peers: None,
        };
        if self.torrent {
            release.info_hash = self.info_hash(item, base)?;
            release.magnet_url = self.magnet_url(item, base)?;
            release.seeders = self.seeders(item)?;
            let peers = self.peers(item)?;
            release.peers = if self.calculate_peers_as_sum {
                match (peers, release.seeders) {
                    (Some(p), Some(s)) => Some(p + s),
                    _ => None,
                }
            } else {
                peers
            };
        }
        Ok(release)
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

    /// `GetEnclosure` (enforcing the preferred types): without preferred types the single
    /// enclosure, and an error when there are several.
    fn enclosure(&self, item: &Element) -> Result<Option<Enclosure>, String> {
        let mut list = Self::enclosures(item);
        if list.is_empty() {
            return Ok(None);
        }
        if let Some(mimes) = self.preferred_enclosure_mime_types {
            for m in mimes {
                if let Some(i) = list.iter().position(|e| e.mime.as_deref() == Some(*m)) {
                    return Ok(Some(list.swap_remove(i)));
                }
            }
            return Ok(None);
        }
        if list.len() > 1 {
            return Err("more than one enclosure".into());
        }
        Ok(list.pop())
    }

    fn download_url(&self, item: &Element, base: &url::Url) -> Result<Option<String>, String> {
        if self.use_enclosure_url {
            return Ok(self
                .enclosure(item)?
                .and_then(|e| parse_url(base, e.url.as_deref())));
        }
        Ok(parse_url(base, item.child("link").map(value).as_deref()))
    }

    /// `GetSize`: the enclosure's length, or the size in the description, or else the size
    /// element.
    fn size(&self, item: &Element) -> Result<i64, String> {
        let mut size = if self.use_enclosure_length {
            self.enclosure(item)?.map_or(0, |e| e.length)
        } else if self.parse_size_in_description
            && let Some(d) = item.child("description")
        {
            parse_size(&value(d), true)?
        } else {
            0
        };
        if size == 0
            && self.torrent
            && let Some(name) = self
                .size_element
                .as_deref()
                .filter(|n| !n.trim().is_empty())
            && let Some(e) = single_descendant(item, name)?
        {
            size = parse_size(&value(e), true)?;
        }
        Ok(size)
    }

    /// `GetInfoHash`: the info-hash element, or the magnet link's hash (upper-case hex).
    fn info_hash(&self, item: &Element, base: &url::Url) -> Result<Option<String>, String> {
        if let Some(name) = self
            .info_hash_element
            .as_deref()
            .filter(|n| !n.trim().is_empty())
        {
            // `FirstOrDefault().Value`: an item without the element fails.
            let e = find_descendants(item, name)
                .into_iter()
                .next()
                .ok_or_else(|| format!("no {name} element"))?;
            return Ok(Some(value(e)));
        }
        Ok(self
            .magnet_url(item, base)?
            .and_then(|m| magnet_info_hash(&m)))
    }

    /// `GetMagnetUrl`: the magnet element, or a download link that is a magnet link.
    fn magnet_url(&self, item: &Element, base: &url::Url) -> Result<Option<String>, String> {
        let url = if let Some(name) = self
            .magnet_element
            .as_deref()
            .filter(|n| !n.trim().is_empty())
        {
            let e = find_descendants(item, name)
                .into_iter()
                .next()
                .ok_or_else(|| format!("no {name} element"))?;
            Some(value(e))
        } else {
            self.download_url(item, base)?
        };
        Ok(url.filter(|u| !u.trim().is_empty() && u.starts_with("magnet:")))
    }

    fn description(item: &Element) -> Option<String> {
        item.child("description").map(value)
    }

    fn seeders(&self, item: &Element) -> Result<Option<i64>, String> {
        if self.parse_seeders_in_description
            && let Some(d) = Self::description(item)
        {
            if let Some(seeders) = labelled_count(&d, "seeder") {
                return parse_int(&seeders).map(Some);
            }
            if let (Some(p), Some(l)) = (labelled_count(&d, "peer"), labelled_count(&d, "leecher"))
            {
                return Ok(Some(parse_int(&p)? - parse_int(&l)?));
            }
        }
        if let Some(name) = self.seeds_element.as_deref()
            && let Some(e) = single_descendant(item, name)?
        {
            return parse_int(&value(e)).map(Some);
        }
        Ok(None)
    }

    fn peers(&self, item: &Element) -> Result<Option<i64>, String> {
        if self.parse_seeders_in_description
            && let Some(d) = Self::description(item)
        {
            if let Some(peers) = labelled_count(&d, "peer") {
                return parse_int(&peers).map(Some);
            }
            if let (Some(s), Some(l)) =
                (labelled_count(&d, "seeder"), labelled_count(&d, "leecher"))
            {
                return Ok(Some(parse_int(&s)? + parse_int(&l)?));
            }
        }
        if let Some(name) = self
            .peers_element
            .as_deref()
            .filter(|n| !n.trim().is_empty())
        {
            let e = single_descendant(item, name)?.ok_or_else(|| format!("no {name} element"))?;
            return parse_int(&value(e)).map(Some);
        }
        Ok(None)
    }
}

// Sonarr's description patterns, matched by hand: `NetRegex` bounds its input length and feed
// descriptions are often longer. Each keeps the .NET pattern's leftmost-match semantics.

/// `\w` (.NET: letters, digits, connector punctuation and marks).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn starts_with_ci(chars: &[char], at: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    chars.len() >= at + w.len()
        && chars[at..at + w.len()]
            .iter()
            .zip(&w)
            .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
}

fn digits_from(chars: &[char], at: usize) -> usize {
    chars.get(at..).map_or(0, |rest| {
        rest.iter().take_while(|c| c.is_ascii_digit()).count()
    })
}

fn spaces_from(chars: &[char], at: usize) -> usize {
    chars.get(at..).map_or(0, |rest| {
        rest.iter().take_while(|c| c.is_whitespace()).count()
    })
}

/// `(Word)s?:\s+(?<value>\d+)|(?<value>\d+)\s+(word)s?` (case-insensitive): the first count
/// labelled with `word` ("Seeders: 6", "3 seeders").
fn labelled_count(text: &str, word: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let len = word.chars().count();
    for i in 0..chars.len() {
        // `Word s? : \s+ \d+`
        if starts_with_ci(&chars, i, word) {
            let mut j = i + len;
            if chars.get(j).is_some_and(|c| c.eq_ignore_ascii_case(&'s'))
                && chars.get(j + 1) == Some(&':')
            {
                j += 1;
            }
            if chars.get(j) == Some(&':') {
                let spaces = spaces_from(&chars, j + 1);
                let at = j + 1 + spaces;
                let digits = digits_from(&chars, at);
                if spaces > 0 && digits > 0 {
                    return Some(chars[at..at + digits].iter().collect());
                }
            }
        }
        // `\d+ \s+ word`
        let digits = digits_from(&chars, i);
        if digits > 0 {
            let spaces = spaces_from(&chars, i + digits);
            if spaces > 0 && starts_with_ci(&chars, i + digits + spaces, word) {
                return Some(chars[i..i + digits].iter().collect());
            }
        }
    }
    None
}

/// The ends of `(?:\d+,)*\d+(?:\.\d{1,3})?` matched at `at`, in backtracking order.
fn size_value_ends(chars: &[char], at: usize, out: &mut Vec<usize>) {
    let run = digits_from(chars, at);
    if run == 0 {
        return;
    }
    // One more `\d+,` iteration first (greedy), then the final `\d+`.
    if chars.get(at + run) == Some(&',') {
        size_value_ends(chars, at + run + 1, out);
    }
    for k in (at + 1..=at + run).rev() {
        if chars.get(k) == Some(&'.') {
            let decimals = digits_from(chars, k + 1).min(3);
            for d in (1..=decimals).rev() {
                out.push(k + 1 + d);
            }
        }
        out.push(k);
    }
}

/// `(?<value>(?<!\.\d*)(?:\d+,)*\d+(?:\.\d{1,3})?)\W?(?<unit>[KMG]i?B)(?![\w/])`
/// (case-insensitive): the first size with a unit, as (value, unit).
fn find_size(text: &str) -> Option<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        if !chars[i].is_ascii_digit() {
            continue;
        }
        // Not the fraction of a number (`(?<!\.\d*)`).
        let before = chars[..i]
            .iter()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if i > before && chars[i - before - 1] == '.' {
            continue;
        }
        let mut ends = Vec::new();
        size_value_ends(&chars, i, &mut ends);
        for end in ends {
            // `\W?`, greedy.
            let separators: &[usize] = if chars.get(end).is_some_and(|c| !is_word(*c)) {
                &[1, 0]
            } else {
                &[0]
            };
            for sep in separators {
                let u = end + sep;
                if !chars
                    .get(u)
                    .is_some_and(|c| matches!(c.to_ascii_uppercase(), 'K' | 'M' | 'G'))
                {
                    continue;
                }
                // `i?`, greedy.
                for with_i in [true, false] {
                    if with_i
                        && !chars
                            .get(u + 1)
                            .is_some_and(|c| c.eq_ignore_ascii_case(&'i'))
                    {
                        continue;
                    }
                    let b = if with_i { u + 2 } else { u + 1 };
                    if !chars.get(b).is_some_and(|c| c.eq_ignore_ascii_case(&'b')) {
                        continue;
                    }
                    if chars.get(b + 1).is_some_and(|c| is_word(*c) || *c == '/') {
                        continue;
                    }
                    return Some((
                        chars[i..end].iter().collect(),
                        chars[u..=b].iter().collect(),
                    ));
                }
            }
        }
    }
    None
}

/// `RssParser.ParseSize`: plain bytes, or the first "1,234.5 MB"-like size in the text. KB/MB/GB
/// are binary with `default_to_binary_prefix`, decimal otherwise; KiB/MiB/GiB always binary.
pub fn parse_size(text: &str, default_to_binary_prefix: bool) -> Result<i64, String> {
    if text.trim().is_empty() {
        return Ok(0);
    }
    if text.chars().all(|c| c.is_ascii_digit()) {
        return text.parse().map_err(|_| format!("invalid size {text:?}"));
    }
    let Some((value, unit)) = find_size(text) else {
        return Ok(0);
    };
    let number: f64 = value
        .replace(',', "")
        .parse()
        .map_err(|_| format!("invalid size {text:?}"))?;
    let (power, binary) = match unit.to_ascii_lowercase().as_str() {
        "kb" => (1, default_to_binary_prefix),
        "mb" => (2, default_to_binary_prefix),
        "gb" => (3, default_to_binary_prefix),
        "kib" => (1, true),
        "mib" => (2, true),
        "gib" => (3, true),
        _ => return Ok(number as i64),
    };
    let prefix: f64 = if binary { 1024.0 } else { 1000.0 };
    // `Convert.ToInt64(double)`: rounded, ties to even.
    Ok((number * prefix.powi(power)).round_ties_even() as i64)
}

/// `MagnetLink.Parse(…).InfoHashes.V1OrV2.ToHex()`: the v1 hash (hex or base32), else the
/// v2 one, as upper-case hex; `None` when the link does not parse.
pub fn magnet_info_hash(magnet: &str) -> Option<String> {
    let query = magnet.strip_prefix("magnet:?")?;
    let (mut v1, mut v2) = (None, None);
    for part in query.split('&') {
        let (key, value) = part.split_once('=')?;
        if value.contains('=') {
            return None;
        }
        if key != "xt" && !key.starts_with("xt.") {
            continue;
        }
        if let Some(h) = value.strip_prefix("urn:btih:") {
            v1 = Some(match h.len() {
                40 => hex_decode(h)?,
                32 => base32_decode(h)?,
                _ => return None,
            });
        } else if let Some(h) = value.strip_prefix("urn:btmh:") {
            // A multihash: 0x12 (SHA-256), 0x20 (32 bytes), then the hash.
            let bytes = hex_decode(h)?;
            if bytes.len() != 34 || bytes[0] != 0x12 || bytes[1] != 0x20 {
                return None;
            }
            v2 = Some(bytes[2..].to_vec());
        }
    }
    let bytes = v1.or(v2)?;
    Some(bytes.iter().map(|b| format!("{b:02X}")).collect())
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// RFC 4648 base32 (no padding), case-insensitive.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u64, 0u32);
    for c in s.bytes() {
        let v = match c.to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | u64::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn items(doc: &Element) -> Vec<&Element> {
    doc.child("channel")
        .map(|c| c.children_named("item").collect())
        .unwrap_or_default()
}

/// `GetTitle`: HTML-decoded, "Unknown" without a title.
fn title(item: &Element) -> String {
    xml::html_decode(&item.child("title").map_or_else(|| "Unknown".into(), value))
}

/// `ParseUrl`: relative links resolved against the request URL; unparseable ones dropped.
fn parse_url(base: &url::Url, value: Option<&str>) -> Option<String> {
    let v = value?.trim();
    if v.is_empty() {
        return None;
    }
    base.join(v).ok().map(|u| u.to_string())
}

// ---- Torrent RSS feed layout detection (`TorrentRssSettingsDetector`) -------------------------

/// `TorrentRssIndexerParserSettings`: the layout detected for a feed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParserSettings {
    pub use_eztv_format: bool,
    pub parse_seeders_in_description: bool,
    pub use_enclosure_url: bool,
    pub use_enclosure_length: bool,
    pub parse_size_in_description: bool,
    pub size_element_name: Option<String>,
}

/// `UnsupportedFeedException`.
fn unsupported(message: impl Into<String>) -> IndexerError {
    IndexerError::Indexer(message.into())
}

fn message(e: &IndexerError) -> String {
    match e {
        IndexerError::ApiKey(m)
        | IndexerError::Indexer(m)
        | IndexerError::Connection(m)
        | IndexerError::InvalidXml(m)
        | IndexerError::Unexpected(m) => m.clone(),
        other => other.to_string(),
    }
}

/// `TorrentRssSettingsDetector.Detect` on the feed's response: the EZRSS layout when the feed
/// declares it and parses as one, otherwise the generic layout, probing where seeders and
/// sizes are.
pub fn detect(
    body: &str,
    request_url: &str,
    allow_zero_size: bool,
) -> Result<ParserSettings, IndexerError> {
    if let Some(settings) = ezrss_settings(body, request_url, allow_zero_size)? {
        return Ok(settings);
    }
    generic_settings(body, request_url, allow_zero_size)
}

fn ezrss_settings(
    body: &str,
    request_url: &str,
    allow_zero_size: bool,
) -> Result<Option<ParserSettings>, IndexerError> {
    if !is_eztv_feed(body)? {
        return Ok(None);
    }
    let releases = parse_response(&RssParser::ezrss(), body, request_url)?;
    match validate_releases(&releases)
        .and_then(|_| validate_release_size(&releases, allow_zero_size))
    {
        Ok(()) => Ok(Some(ParserSettings {
            use_eztv_format: true,
            ..ParserSettings::default()
        })),
        Err(e) => {
            tracing::trace!(error = %e, "feed wasn't parsable by the EZRSS parser");
            Ok(None)
        }
    }
}

fn generic_settings(
    body: &str,
    request_url: &str,
    allow_zero_size: bool,
) -> Result<ParserSettings, IndexerError> {
    let doc = xml::parse(body).map_err(IndexerError::InvalidXml)?;
    let item = items(&doc)
        .first()
        .copied()
        .ok_or_else(|| unsupported("Empty feed, cannot check if feed is parsable."))?;
    let mut settings = ParserSettings {
        use_enclosure_url: true,
        use_enclosure_length: true,
        parse_seeders_in_description: true,
        ..ParserSettings::default()
    };
    if item.child("enclosure").is_none() {
        settings.use_enclosure_url = false;
    }
    let parse = |s: &ParserSettings| -> Result<Vec<Release>, IndexerError> {
        let parser = RssParser {
            use_enclosure_url: s.use_enclosure_url,
            use_enclosure_length: s.use_enclosure_length,
            parse_seeders_in_description: s.parse_seeders_in_description,
            parse_size_in_description: s.parse_size_in_description,
            size_element: s.size_element_name.clone(),
            ..RssParser::torrent()
        };
        let releases = parse_response(&parser, body, request_url)?;
        validate_releases(&releases)?;
        Ok(releases)
    };
    let small = |releases: &[Release]| releases.iter().any(|r| r.size < VALID_SIZE_THRESHOLD);

    let releases = parse(&settings)?;
    if !releases.iter().any(|r| r.seeders.is_some()) {
        settings.parse_seeders_in_description = false;
    }
    if !small(&releases) {
        // The enclosure carries valid sizes.
        return Ok(settings);
    }
    settings.use_enclosure_length = false;
    for name in ["size", "Size"] {
        settings.size_element_name = Some(name.into());
        if !small(&parse(&settings)?) {
            return Ok(settings);
        }
    }
    settings.size_element_name = None;
    settings.parse_size_in_description = true;
    let releases = parse(&settings)?;
    let valid = releases
        .iter()
        .filter(|r| r.size >= VALID_SIZE_THRESHOLD)
        .count();
    if valid > releases.len() / 2 {
        if small(&releases) {
            tracing::debug!(url = request_url, "feed contains very small releases");
        }
        return Ok(settings);
    }
    settings.parse_size_in_description = false;
    tracing::debug!(url = request_url, "feed doesn't have release sizes");
    let releases = parse(&settings)?;
    validate_release_size(&releases, allow_zero_size)?;
    Ok(settings)
}

/// `IsEZTVFeed`: the EZRSS namespace on the root's `torrent` prefix, the EZRSS DTD, or an
/// element in the EZRSS namespace.
fn is_eztv_feed(body: &str) -> Result<bool, IndexerError> {
    let doc = xml::parse(body).map_err(IndexerError::InvalidXml)?;
    let root_tag = body
        .find(&format!("<{}", doc.name))
        .and_then(|start| body[start..].find('>').map(|end| &body[start..start + end]))
        .unwrap_or_default();
    if root_tag.contains(&format!("xmlns:torrent=\"{EZRSS_NS}\""))
        || root_tag.contains(&format!("xmlns:torrent='{EZRSS_NS}'"))
    {
        return Ok(true);
    }
    if let Some(start) = body.find("<!DOCTYPE")
        && let Some(end) = body[start..].find('>')
        && body[start..start + end].contains("\"http://xmlns.ezrss.it/0.1/dtd/\"")
    {
        return Ok(true);
    }
    fn any_ezrss(e: &Element) -> bool {
        e.ns == EZRSS_NS || e.children.iter().any(any_ezrss)
    }
    Ok(any_ezrss(&doc))
}

/// `ParseResponse`: any parse failure makes the feed unsupported.
fn parse_response(
    parser: &RssParser,
    body: &str,
    request_url: &str,
) -> Result<Vec<Release>, IndexerError> {
    parser
        .parse(body, request_url)
        .map_err(|e| unsupported(format!("Unable to parse indexer: {}", message(&e))))
}

fn is_valid_download_url(url: Option<&str>) -> bool {
    url.is_some_and(|u| {
        !u.trim().is_empty()
            && (u.starts_with("magnet:") || u.starts_with("http:") || u.starts_with("https:"))
    })
}

fn validate_releases(releases: &[Release]) -> Result<(), IndexerError> {
    if releases.is_empty() {
        return Err(unsupported("Empty feed, cannot check if feed is parsable."));
    }
    if releases.iter().any(|r| r.title.trim().is_empty()) {
        return Err(unsupported("Feed contains releases without title."));
    }
    if releases
        .iter()
        .any(|r| !is_valid_download_url(r.download_url.as_deref()))
    {
        return Err(unsupported(
            "Failed to find a valid download url in the feed.",
        ));
    }
    let mut guids: Vec<&str> = releases.iter().map(|r| r.guid.as_str()).collect();
    let total = guids.len();
    guids.sort_unstable();
    guids.dedup();
    if guids.len() != total {
        return Err(unsupported(
            "Feed contains releases with same guid, rejecting malformed rss feed.",
        ));
    }
    Ok(())
}

fn validate_release_size(releases: &[Release], allow_zero_size: bool) -> Result<(), IndexerError> {
    if !allow_zero_size && releases.iter().any(|r| r.size == 0) {
        return Err(unsupported(
            "Feed doesn't contain the release content size.",
        ));
    }
    if releases
        .iter()
        .any(|r| r.size != 0 && r.size < VALID_SIZE_THRESHOLD)
    {
        return Err(unsupported(
            "Size of one more releases lower than 2.0 MB, feed must contain release content size.",
        ));
    }
    Ok(())
}

// ---- Parser factory (`TorrentRssParserFactory`) -----------------------------------------------

/// The parser for a detected layout (`TorrentRssParserFactory.GetParser`).
pub fn parser_for(settings: &ParserSettings) -> RssParser {
    if settings.use_eztv_format {
        return RssParser::ezrss();
    }
    RssParser {
        use_guid_info_url: false,
        parse_seeders_in_description: settings.parse_seeders_in_description,
        use_enclosure_url: settings.use_enclosure_url,
        use_enclosure_length: settings.use_enclosure_length,
        parse_size_in_description: settings.parse_size_in_description,
        size_element: settings.size_element_name.clone(),
        ..RssParser::torrent()
    }
}

/// Detected layouts per indexer settings, kept for a week. Failed detections are not cached.
#[derive(Debug, Default)]
pub struct ParserFactory {
    cache: Mutex<HashMap<String, (Instant, ParserSettings)>>,
}

impl ParserFactory {
    /// The parser for the indexer settings `key`, detecting the layout with `detect` unless a
    /// detection for the same settings is cached.
    pub fn get_parser(
        &self,
        key: &str,
        detect: impl FnOnce() -> Result<ParserSettings, IndexerError>,
    ) -> Result<RssParser, IndexerError> {
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .filter(|(at, _)| at.elapsed() < PARSER_SETTINGS_TTL)
            .map(|(_, s)| s.clone());
        let settings = match cached {
            Some(s) => s,
            None => {
                let s = detect()?;
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.to_string(), (Instant::now(), s.clone()));
                s
            }
        };
        Ok(parser_for(&settings))
    }
}

// ---- Requests and settings of the RSS-only indexers -------------------------------------------

/// `HttpHeader.ParseCookies`: `a=b; c=d` as a `Cookie` header.
fn cookie_header(cookies: &str) -> Option<String> {
    let pairs: Vec<String> = cookies
        .split(';')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .filter_map(|c| c.split_once('=').map(|(k, v)| format!("{k}={v}")))
        .collect();
    (!pairs.is_empty()).then(|| pairs.join("; "))
}

/// `GetRecentRequests` of the Torrent RSS, IPTorrents and TorrentLeech indexers: the feed.
pub(crate) fn recent(dialect: Dialect, settings: &Settings) -> Chain {
    let url = match dialect {
        Dialect::IpTorrents => settings.base_url.clone(),
        Dialect::Torrentleech => format!(
            "{}/{}",
            settings.base_url.trim().trim_end_matches('/'),
            settings.api_key.as_deref().unwrap_or_default()
        ),
        _ => settings.base_url.trim().trim_end_matches('/').to_string(),
    };
    let mut request = Request::get(url, ACCEPT_RSS);
    if dialect == Dialect::TorrentRss {
        request.cookie = settings.cookie.as_deref().and_then(cookie_header);
    }
    let mut chain = Chain::default();
    chain.add(Pageable::of(vec![request], 0));
    chain
}

static IPT_FEED: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"(?:/|t\.)rss\?.+$", false));
static IPT_DOWNLOAD: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"(?:/|t\.)rss\?.+;download(?:;|$)", false));

/// `IPTorrentsSettingsValidator`: the feed URL, with `;download` for direct download links.
pub fn validate_iptorrents_url(url: &str) -> Result<(), String> {
    if !IPT_FEED.is_match(url) {
        return Err("Use the RSS feed URL (…/t.rss?u=…;tp=…)".into());
    }
    if !IPT_DOWNLOAD.is_match(url) {
        return Err("Use Direct Download Url (;download)".into());
    }
    Ok(())
}
