//! Metadata providers.
//!
//! Each adapter maps its catalogue into the same [`SeriesInfo`] / [`SearchResult`] model and
//! keeps the provider's native identity. Requests are rate limited per provider, retried once
//! on HTTP 429 (honouring `Retry-After`) and cached briefly in memory.
//!
//! Implemented: TVmaze (TV, no key) and AniList (anime and anime films, no key). Other
//! catalogues are tracked in the feature matrix.

pub mod anilist;
mod cache;
mod ratelimit;
pub mod tvmaze;

use std::time::Duration;

use kitsunarr_domain::{ExternalId, MediaImage, MetadataProvider, SeriesStatus, SeriesType};
use serde::{Deserialize, Serialize};

pub use cache::TtlCache;
pub use ratelimit::RateLimiter;

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("{0} is not supported yet")]
    Unsupported(MetadataProvider),
    #[error("not found in {0}")]
    NotFound(MetadataProvider),
    #[error("{provider} request failed: {message}")]
    Http {
        provider: MetadataProvider,
        message: String,
    },
    #[error("{provider} returned unexpected data: {message}")]
    Decode {
        provider: MetadataProvider,
        message: String,
    },
}

pub type MetadataResult<T> = Result<T, MetadataError>;

pub use kitsunarr_domain::api::metadata::{CatalogueInfo, EntryKind};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub provider: MetadataProvider,
    pub id: String,
    pub kind: EntryKind,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub status: SeriesStatus,
    pub poster_url: Option<String>,
    pub external_ids: Vec<ExternalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeInfo {
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub title: Option<String>,
    pub overview: Option<String>,
    /// Local air date (`YYYY-MM-DD`).
    pub air_date: Option<String>,
    /// RFC 3339 UTC.
    pub air_date_utc: Option<String>,
    pub runtime: Option<i32>,
    pub provider_episode_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonInfo {
    pub season_number: i32,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesInfo {
    pub provider: MetadataProvider,
    pub id: String,
    pub kind: EntryKind,
    pub title: String,
    pub alternate_titles: Vec<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub status: SeriesStatus,
    pub series_type: SeriesType,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub air_time: Option<String>,
    pub runtime: Option<i32>,
    pub genres: Vec<String>,
    pub images: Vec<MediaImage>,
    pub rating: Option<f64>,
    pub votes: Option<i64>,
    pub first_aired: Option<String>,
    pub last_aired: Option<String>,
    /// Kitsunarr language id.
    pub original_language: Option<i32>,
    pub external_ids: Vec<ExternalId>,
    pub seasons: Vec<SeasonInfo>,
    pub episodes: Vec<EpisodeInfo>,
}

/// Removes HTML tags and decodes the handful of entities catalogues use.
pub fn strip_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Provider registry with shared HTTP client, rate limits and caches.
#[derive(Clone)]
pub struct Metadata {
    pub tvmaze: tvmaze::TvMaze,
    pub anilist: anilist::AniList,
}

impl Metadata {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_endpoints(http, tvmaze::DEFAULT_BASE, anilist::DEFAULT_BASE)
    }

    /// Uses alternative endpoints (a caching proxy, a mirror, or a local mock in tests).
    pub fn with_endpoints(http: reqwest::Client, tvmaze_url: &str, anilist_url: &str) -> Self {
        Self {
            tvmaze: tvmaze::TvMaze::with_base(http.clone(), tvmaze_url),
            anilist: anilist::AniList::with_base(http, anilist_url),
        }
    }

    pub fn supports(provider: MetadataProvider) -> bool {
        kitsunarr_domain::api::metadata::SUPPORTED_CATALOGUES.contains(&provider)
    }

    /// The catalogues that can be searched, with the credit their terms ask for.
    pub fn catalogues() -> Vec<CatalogueInfo> {
        vec![
            CatalogueInfo {
                provider: MetadataProvider::TvMaze,
                name: "TVmaze".into(),
                description: "TV series and their episode guides".into(),
                kinds: vec![EntryKind::Series],
                attribution: "TV data provided by TVmaze.com under CC BY-SA 4.0".into(),
                url: "https://www.tvmaze.com".into(),
            },
            CatalogueInfo {
                provider: MetadataProvider::AniList,
                name: "AniList".into(),
                description: "Anime series and anime films".into(),
                kinds: vec![EntryKind::Anime, EntryKind::Movie],
                attribution: "Anime data provided by AniList".into(),
                url: "https://anilist.co".into(),
            },
        ]
    }

    pub async fn search(
        &self,
        provider: MetadataProvider,
        term: &str,
    ) -> MetadataResult<Vec<SearchResult>> {
        match provider {
            MetadataProvider::TvMaze => self.tvmaze.search(term).await,
            MetadataProvider::AniList => self.anilist.search(term).await,
            other => Err(MetadataError::Unsupported(other)),
        }
    }

    pub async fn series(&self, provider: MetadataProvider, id: &str) -> MetadataResult<SeriesInfo> {
        match provider {
            MetadataProvider::TvMaze => self.tvmaze.series(id).await,
            MetadataProvider::AniList => self.anilist.series(id).await,
            other => Err(MetadataError::Unsupported(other)),
        }
    }

    /// Like [`Self::series`] but bypasses the in-memory cache (used by refreshes).
    pub async fn series_fresh(
        &self,
        provider: MetadataProvider,
        id: &str,
    ) -> MetadataResult<SeriesInfo> {
        match provider {
            MetadataProvider::TvMaze => self.tvmaze.forget(id),
            MetadataProvider::AniList => self.anilist.forget(id),
            _ => {}
        }
        self.series(provider, id).await
    }
}

/// Sends a request with rate limiting and one retry on HTTP 429.
pub(crate) async fn send_json<T: serde::de::DeserializeOwned>(
    provider: MetadataProvider,
    limiter: &RateLimiter,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> MetadataResult<T> {
    for attempt in 0..2 {
        limiter.acquire().await;
        let resp = build()
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| MetadataError::Http {
                provider,
                message: e.to_string(),
            })?;
        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt == 0 {
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5)
                .min(60);
            tracing::warn!(%provider, wait, "rate limited by provider; retrying");
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(MetadataError::NotFound(provider));
        }
        if !status.is_success() {
            return Err(MetadataError::Http {
                provider,
                message: format!("HTTP {status}"),
            });
        }
        let text = resp.text().await.map_err(|e| MetadataError::Http {
            provider,
            message: e.to_string(),
        })?;
        return serde_json::from_str(&text).map_err(|e| MetadataError::Decode {
            provider,
            message: e.to_string(),
        });
    }
    Err(MetadataError::Http {
        provider,
        message: "rate limited".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_is_stripped() {
        assert_eq!(
            strip_html("<p><b>Walter</b> &amp; Jesse</p>\n<br/> cook."),
            "Walter & Jesse cook."
        );
        assert_eq!(strip_html("a &lt; b"), "a < b");
    }
}
