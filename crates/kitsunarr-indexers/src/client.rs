//! Fetching from indexers (`HttpIndexerBase.cs`, `NewznabCapabilitiesProvider.cs`), with each
//! dialect's response checks and parser.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::caps::{Capabilities, capabilities_or_default};
use crate::error::IndexerError;
use crate::feed::{Dialect, Release, is_valid, parse_feed};
use crate::request::{ACCEPT_RSS, Chain, Generator, Method, Request, Settings};
use crate::torrent_rss::ParserFactory;

/// Minimum spacing between two requests to the same indexer (`HttpIndexerBase.RateLimit`).
pub const RATE_LIMIT: Duration = Duration::from_secs(2);
/// Stop paging a query once this many releases were collected (`MaxNumResultsPerQuery`).
pub const MAX_RESULTS_PER_QUERY: usize = 1000;
/// Capabilities are cached for a week (`NewznabCapabilitiesProvider`).
const CAPS_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// An indexer's answer, before the dialect's checks.
struct Response {
    status: u16,
    content_type: String,
    body: String,
}

/// The innermost cause of a request error (e.g. "failed to lookup address information"),
/// without the URL, which may contain the API key.
fn error_chain(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    if e.is_timeout() {
        return "the request timed out".into();
    }
    let mut innermost = None;
    let mut cause = e.source();
    while let Some(c) = cause {
        innermost = Some(c.to_string());
        cause = c.source();
    }
    match (innermost, e.url()) {
        (Some(m), _) if !m.is_empty() => m,
        (_, Some(url)) => e.to_string().replace(url.as_str(), "<redacted>"),
        _ => e.to_string(),
    }
}

/// A configured indexer.
#[derive(Debug, Clone)]
pub struct Indexer {
    pub id: i64,
    pub name: String,
    pub dialect: Dialect,
    pub settings: Settings,
}

/// How [`IndexerClient::fetch_with`] pages.
#[derive(Clone, Copy)]
enum Mode<'a> {
    Search,
    FirstPage,
    Recent(Option<&'a LastRelease>),
}

/// The newest release of an RSS sync (Sonarr's `LastRssSyncReleaseInfo`), kept by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRelease {
    pub publish_date: chrono::DateTime<chrono::Utc>,
    pub download_url: Option<String>,
    pub guid: String,
    pub title: String,
}

impl LastRelease {
    pub fn of(r: &Release) -> Self {
        Self {
            publish_date: r.publish_date,
            download_url: r.download_url.clone(),
            guid: r.guid.clone(),
            title: r.title.clone(),
        }
    }
}

/// What an RSS sync read from one indexer.
#[derive(Debug, Clone)]
pub struct RecentFetch {
    pub releases: Vec<IndexerRelease>,
    /// The newest release read, to remember for the next sync.
    pub newest: Option<LastRelease>,
    /// The period between the previous sync and the oldest release read, when paging stopped
    /// before reaching the previous sync.
    pub gap: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
}

/// A release with the indexer it came from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerRelease {
    pub indexer_id: i64,
    pub indexer: String,
    #[serde(flatten)]
    pub release: Release,
}

/// Outcome of testing an indexer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub releases_found: usize,
    pub warnings: Vec<String>,
    pub capabilities: Capabilities,
}

/// What a release's download link gave (`DownloadFromWebUrl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Downloaded {
    /// A `.torrent` (or `.nzb`) file.
    File(Vec<u8>),
    /// The indexer redirected to a magnet link.
    Magnet(String),
}

#[derive(Clone, Default)]
pub struct IndexerClient {
    http: reqwest::Client,
    /// Without automatic redirects: a download link may redirect to a magnet link.
    downloads: reqwest::Client,
    rate_limit: Duration,
    next_request: Arc<Mutex<HashMap<i64, Instant>>>,
    caps: Arc<Mutex<HashMap<String, (Instant, Capabilities)>>>,
    /// Torrent RSS feed layouts, detected from each feed's first response.
    torrent_rss: Arc<ParserFactory>,
}

impl IndexerClient {
    pub fn new(http: reqwest::Client) -> Self {
        let downloads = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Kitsunarr/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            http,
            downloads,
            rate_limit: RATE_LIMIT,
            ..Default::default()
        }
    }

    /// Sends downloads through this proxy (the search client is built by the caller).
    pub fn with_download_proxy(mut self, proxy: reqwest::Proxy) -> Self {
        if let Ok(downloads) = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Kitsunarr/", env!("CARGO_PKG_VERSION")))
            .proxy(proxy)
            .build()
        {
            self.downloads = downloads;
        }
        self
    }

    /// Overrides the per-indexer request spacing (tests).
    pub fn with_rate_limit(mut self, rate_limit: Duration) -> Self {
        self.rate_limit = rate_limit;
        self
    }

    /// The spacing between two requests to an indexer: BroadcasTheNet asks for 5 s instead of
    /// 2 s (`RateLimit`), scaled with the configured spacing.
    fn spacing(&self, dialect: Dialect) -> Duration {
        if dialect == Dialect::BroadcastheNet {
            self.rate_limit.mul_f64(2.5)
        } else {
            self.rate_limit
        }
    }

    async fn wait_turn(&self, indexer_id: i64) {
        self.wait_turn_for(indexer_id, self.rate_limit).await;
    }

    async fn wait_turn_for(&self, indexer_id: i64, spacing: Duration) {
        let wait = {
            let mut map = self.next_request.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let slot = map
                .get(&indexer_id)
                .copied()
                .filter(|t| *t > now)
                .unwrap_or(now);
            map.insert(indexer_id, slot + spacing);
            slot.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Sends a request in the indexer's turn. Connection failures and HTTP 429 are errors;
    /// other statuses are left to the dialect's checks.
    async fn send(&self, indexer: &Indexer, request: &Request) -> Result<Response, IndexerError> {
        self.wait_turn_for(indexer.id, self.spacing(indexer.dialect))
            .await;
        let mut builder = match request.method {
            Method::Get => self.http.get(&request.url),
            Method::Post => self.http.post(&request.url),
        }
        .header(reqwest::header::ACCEPT, request.accept)
        .timeout(Duration::from_secs(60));
        if let Some(body) = &request.body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone());
        }
        if let Some((user, password)) = &request.basic_auth {
            builder = builder.basic_auth(user, Some(password));
        }
        if let Some(cookie) = &request.cookie {
            builder = builder.header(reqwest::header::COOKIE, cookie);
        }
        let response = builder.send().await.map_err(|e| {
            let message = error_chain(&e);
            if e.is_connect() || e.is_timeout() {
                IndexerError::Connection(message)
            } else {
                IndexerError::Unexpected(message)
            }
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after_secs = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok());
            return Err(IndexerError::RequestLimit { retry_after_secs });
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let body = response
            .text()
            .await
            .map_err(|e| IndexerError::Unexpected(e.without_url().to_string()))?;
        Ok(Response {
            status: status.as_u16(),
            content_type,
            body,
        })
    }

    /// The checks of an RSS/XML response (`RssParser.PreProcess` and the HTTP client).
    fn check_xml(response: &Response) -> Result<(), IndexerError> {
        let status = reqwest::StatusCode::from_u16(response.status)
            .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        let content_type = &response.content_type;
        if !status.is_success() {
            // An XML body with an error status may carry an <error> element (checked by the parser).
            if !content_type.contains("xml") {
                return Err(IndexerError::Http {
                    status: status.as_u16(),
                });
            }
        } else if content_type.contains("text/html") {
            return Err(IndexerError::Unexpected(
                "the indexer responded with HTML content; it may be offline, behind a login, or the URL is wrong".into(),
            ));
        }
        Ok(())
    }

    /// A GET of an RSS/XML document (capabilities).
    async fn get(&self, indexer: &Indexer, url: &str) -> Result<String, IndexerError> {
        let response = self.send(indexer, &Request::get(url, ACCEPT_RSS)).await?;
        Self::check_xml(&response)?;
        Ok(response.body)
    }

    /// One page: the request, the dialect's response checks and its parser.
    async fn fetch_page(
        &self,
        indexer: &Indexer,
        request: &Request,
    ) -> Result<Vec<Release>, IndexerError> {
        let response = self.send(indexer, request).await?;
        let url = request.url.as_str();
        match indexer.dialect {
            Dialect::FileList => {
                if !(200..300).contains(&response.status) {
                    return Err(IndexerError::Http {
                        status: response.status,
                    });
                }
                crate::filelist::check_response(response.status, &response.content_type)?;
                crate::filelist::parse(&response.body, &indexer.settings)
            }
            Dialect::HdBits => {
                if !(200..300).contains(&response.status) {
                    return Err(IndexerError::Http {
                        status: response.status,
                    });
                }
                crate::hdbits::parse(&response.body, &indexer.settings)
            }
            Dialect::BroadcastheNet => {
                crate::broadcasthenet::check_response(
                    response.status,
                    &response.content_type,
                    &response.body,
                )?;
                crate::broadcasthenet::parse(&response.body, url)
            }
            Dialect::TorrentRss => {
                Self::check_xml(&response)?;
                // `TorrentRssParserFactory`: the layout is detected once per settings (from
                // this response rather than an extra request) and kept for a week.
                let key = serde_json::to_string(&indexer.settings).unwrap_or_default();
                let parser = self.torrent_rss.get_parser(&key, || {
                    crate::torrent_rss::detect(
                        &response.body,
                        url,
                        indexer.settings.allow_zero_size,
                    )
                })?;
                parser.parse(&response.body, url)
            }
            Dialect::Newznab
            | Dialect::Torznab
            | Dialect::Nyaa
            | Dialect::IpTorrents
            | Dialect::Torrentleech
            | Dialect::Fanzub => {
                Self::check_xml(&response)?;
                parse_feed(&response.body, indexer.dialect, url)
            }
        }
    }

    /// Downloads a release's file, following redirects by hand (at most five) so that a
    /// redirect to a magnet link is recognised. Requests share the indexer's rate limit.
    pub async fn download(
        &self,
        indexer_id: i64,
        url: &str,
        accept: &str,
    ) -> Result<Downloaded, IndexerError> {
        let mut current = url.to_string();
        for _ in 0..5 {
            self.wait_turn(indexer_id).await;
            let response = self
                .downloads
                .get(&current)
                .header(reqwest::header::ACCEPT, accept)
                .timeout(Duration::from_secs(60))
                .send()
                .await
                .map_err(|e| IndexerError::Connection(error_chain(&e)))?;
            let status = response.status();
            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        IndexerError::Unexpected("the indexer redirected without a location".into())
                    })?;
                if location.starts_with("magnet:") {
                    return Ok(Downloaded::Magnet(location.to_string()));
                }
                current = url::Url::parse(&current)
                    .and_then(|base| base.join(location))
                    .map_err(|e| IndexerError::Unexpected(format!("invalid redirect: {e}")))?
                    .to_string();
                continue;
            }
            match status {
                reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE => {
                    return Err(IndexerError::Indexer(
                        "the release is no longer available on the indexer".into(),
                    ));
                }
                reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    return Err(IndexerError::RequestLimit {
                        retry_after_secs: None,
                    });
                }
                s if !s.is_success() => {
                    return Err(IndexerError::Http { status: s.as_u16() });
                }
                _ => {}
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|e| IndexerError::Connection(error_chain(&e)))?;
            return Ok(Downloaded::File(bytes.to_vec()));
        }
        Err(IndexerError::Unexpected("too many redirects".into()))
    }

    /// Capabilities, cached per settings for a week. Failures are not cached.
    pub async fn capabilities(&self, indexer: &Indexer) -> Result<Capabilities, IndexerError> {
        // Only Newznab and Torznab have a capabilities document; the others search their own
        // way (Nyaa's RSS search takes a plain term).
        if !indexer.dialect.has_capabilities() {
            return Ok(Capabilities::default());
        }
        let key = serde_json::to_string(&indexer.settings).unwrap_or_default();
        if let Some((at, caps)) = self
            .caps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
            && at.elapsed() < CAPS_TTL
        {
            return Ok(caps);
        }
        let body = self.get(indexer, &indexer.settings.caps_url()).await?;
        let caps = capabilities_or_default(&body)?;
        self.caps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (Instant::now(), caps.clone()));
        Ok(caps)
    }

    /// Request generator for an indexer, with its page size.
    pub fn generator<'a>(&self, indexer: &'a Indexer, caps: &'a Capabilities) -> Generator<'a> {
        Generator {
            settings: &indexer.settings,
            caps,
            page_size: u32::try_from(caps.page_size()).unwrap_or(100),
            dialect: indexer.dialect,
        }
    }

    /// `FetchReleases` for searches: pages each request until a page is not full, stops after
    /// the first tier that found anything, and returns valid releases without duplicate GUIDs.
    /// With `first_page_only` (an indexer test) each request reads its newest page.
    pub async fn fetch(
        &self,
        indexer: &Indexer,
        chain: &Chain,
        first_page_only: bool,
    ) -> Result<Vec<IndexerRelease>, IndexerError> {
        let mode = if first_page_only {
            Mode::FirstPage
        } else {
            Mode::Search
        };
        Ok(self.fetch_with(indexer, chain, mode).await?.0)
    }

    /// `FetchReleases` for an RSS sync: pages the feed back until it reaches `last` (the newest
    /// release of the previous sync, by date or download link), or reads the newest page when
    /// there was none. Returns the newest release to remember, and the period not covered
    /// when 1,000 releases older than a day were read without reaching `last`.
    pub async fn fetch_recent(
        &self,
        indexer: &Indexer,
        chain: &Chain,
        last: Option<&LastRelease>,
    ) -> Result<RecentFetch, IndexerError> {
        // BroadcasTheNet reads from just before the last torrent seen (`LastRecentTorrentId`).
        let btn_chain;
        let chain = match last.and_then(|l| crate::broadcasthenet::torrent_id(&l.guid)) {
            Some(id) if indexer.dialect == Dialect::BroadcastheNet => {
                btn_chain = crate::broadcasthenet::recent(&indexer.settings, Some(id));
                &btn_chain
            }
            _ => chain,
        };
        let (releases, fully_updated) = self.fetch_with(indexer, chain, Mode::Recent(last)).await?;
        let newest = releases
            .iter()
            .max_by_key(|r| r.release.publish_date)
            .map(|r| LastRelease::of(&r.release));
        let gap = match (last, releases.iter().map(|r| r.release.publish_date).min()) {
            (Some(l), Some(oldest)) if !fully_updated => {
                tracing::warn!(
                    indexer = %indexer.name,
                    from = %l.publish_date,
                    to = %oldest,
                    "RSS sync didn't cover the period between the last sync and the oldest release read; a search may be required"
                );
                Some((l.publish_date, oldest))
            }
            _ => None,
        };
        Ok(RecentFetch {
            releases,
            newest,
            gap,
        })
    }

    async fn fetch_with(
        &self,
        indexer: &Indexer,
        chain: &Chain,
        mode: Mode<'_>,
    ) -> Result<(Vec<IndexerRelease>, bool), IndexerError> {
        let mut releases: Vec<Release> = Vec::new();
        let mut fully_updated = false;
        let now = chrono::Utc::now();
        for tier in &chain.tiers {
            for pageable in tier {
                let mut paged: Vec<Release> = Vec::new();
                for request in pageable.requests() {
                    let page = self.fetch_page(indexer, &request).await?;
                    let full =
                        pageable.page_size() != 0 && page.len() >= pageable.page_size() as usize;
                    let oldest = page.iter().map(|r| r.publish_date).min();
                    let reached = match mode {
                        Mode::Recent(Some(l)) => {
                            oldest.is_some_and(|o| o < l.publish_date)
                                || page.iter().any(|r| {
                                    r.download_url.is_some() && r.download_url == l.download_url
                                })
                        }
                        _ => false,
                    };
                    paged.extend(page);
                    match (mode, oldest) {
                        (Mode::FirstPage, _) => break,
                        (Mode::Recent(None), Some(_)) => {
                            fully_updated = true;
                            break;
                        }
                        (Mode::Recent(Some(_)), Some(oldest)) => {
                            if reached {
                                fully_updated = true;
                                break;
                            }
                            if paged.len() >= MAX_RESULTS_PER_QUERY
                                && oldest < now - chrono::Duration::hours(24)
                            {
                                fully_updated = false;
                                break;
                            }
                        }
                        _ if paged.len() >= MAX_RESULTS_PER_QUERY => break,
                        _ => {}
                    }
                    if !full {
                        break;
                    }
                }
                releases.extend(paged.into_iter().filter(is_valid));
            }
            if !releases.is_empty() {
                break;
            }
        }
        let mut seen = std::collections::HashSet::new();
        let releases = releases
            .into_iter()
            .filter(|r| seen.insert(r.guid.clone()))
            .map(|release| IndexerRelease {
                indexer_id: indexer.id,
                indexer: indexer.name.clone(),
                release,
            })
            .collect();
        Ok((releases, fully_updated))
    }

    /// `Test`: the newest page must return releases in the configured categories and the
    /// capabilities must allow TV searches.
    pub async fn test(&self, indexer: &Indexer) -> Result<TestReport, IndexerError> {
        let caps = self.capabilities(indexer).await?;
        let chain = self.generator(indexer, &caps).recent();
        let found = self.fetch(indexer, &chain, true).await?.len();
        if found == 0 {
            return Err(IndexerError::Indexer(
                "the query succeeded but returned no results in the configured categories; check the categories".into(),
            ));
        }
        if !caps.usable_for_tv() {
            return Err(IndexerError::Indexer(
                "the indexer does not support the required search parameters (q, or tvdbid/rid with season and ep)".into(),
            ));
        }
        let mut warnings = Vec::new();
        if indexer.dialect == Dialect::Torznab && indexer.settings.is_jackett_all() {
            warnings.push(
                "Jackett's \"all\" endpoint is not recommended: add each tracker as its own indexer so failures and limits are tracked separately".into(),
            );
        }
        Ok(TestReport {
            releases_found: found,
            warnings,
            capabilities: caps,
        })
    }
}
