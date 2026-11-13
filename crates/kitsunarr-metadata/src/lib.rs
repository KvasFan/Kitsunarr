//! Metadata providers.
//!
//! Each adapter maps its catalogue into the same [`SeriesInfo`] / [`SearchResult`] model and
//! keeps the provider's native identity. Requests are rate limited per provider, retried once
//! on HTTP 429 (honouring `Retry-After`) and cached briefly in memory.
//!
//! Implemented: TheTVDB through SkyHook (TV and anime, no key), TMDB (TV and films, the user's
//! API key), TVmaze (TV, no key), AniList (anime and anime films, no key), MyAnimeList through
//! Jikan (anime and anime films, no key) and OMDb (films, the user's API key).

pub mod anilist;
mod cache;
pub mod jikan;
pub mod omdb;
mod ratelimit;
pub mod skyhook;
pub mod tmdb;
pub mod tvmaze;
pub mod xem;

use std::time::Duration;

use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MetadataProvider, SeriesStatus, SeriesType,
};
use serde::{Deserialize, Serialize};

pub use cache::TtlCache;
pub use ratelimit::RateLimiter;

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("{0} is not supported yet")]
    Unsupported(MetadataProvider),
    #[error("{0} needs an API key; enter it in Settings › Metadata")]
    MissingKey(MetadataProvider),
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
    /// The episode's screenshot (Sonarr's `MediaCoverTypes.Screenshot`), for metadata files.
    pub image: Option<String>,
}

/// A TMDB film collection, its films ordered by release date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieCollection {
    pub id: i64,
    pub name: String,
    pub overview: Option<String>,
    /// TMDB movie ids.
    pub parts: Vec<i64>,
}

/// An anime and its franchise relations (for automatic grouping).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnimeNode {
    pub id: i64,
    /// `TV`, `TV_SHORT`, `ONA`, `MOVIE`, `OVA`, `SPECIAL`, ...
    pub format: String,
    pub title: String,
    pub relations: Vec<AnimeRelation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimeRelation {
    /// `PREQUEL`, `SEQUEL`, `PARENT` or `SIDE_STORY`.
    pub kind: String,
    pub id: i64,
    pub format: String,
    pub title: String,
}

/// An episode ordering a catalogue offers for a series (`id` is stored in
/// `series.episode_ordering`; the empty id is the catalogue's default order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeOrdering {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonInfo {
    pub season_number: i32,
    pub title: Option<String>,
    /// Season artwork (poster, banner), for metadata files.
    pub images: Vec<MediaImage>,
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
    /// ISO 3166 country code of origin as the catalogue gives it (Sonarr's `OriginalCountry`).
    pub original_country: Option<String>,
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

/// The search result describing a title fetched in full.
pub(crate) fn result_of(info: &SeriesInfo) -> SearchResult {
    SearchResult {
        provider: info.provider,
        id: info.id.clone(),
        kind: info.kind,
        title: info.title.clone(),
        year: info.year,
        overview: info.overview.clone(),
        network: info.network.clone(),
        status: info.status,
        poster_url: info
            .images
            .iter()
            .find(|i| i.cover_type == "poster")
            .map(|i| i.remote_url.clone()),
        external_ids: info.external_ids.clone(),
    }
}

/// Where each catalogue is reached (a caching proxy, a mirror, or a local mock in tests).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub tvmaze: String,
    pub anilist: String,
    pub tvdb: String,
    pub tmdb: String,
    pub jikan: String,
    pub omdb: String,
    /// TheXEM, for scene numbering.
    pub xem: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            tvmaze: tvmaze::DEFAULT_BASE.into(),
            anilist: anilist::DEFAULT_BASE.into(),
            tvdb: skyhook::DEFAULT_BASE.into(),
            tmdb: tmdb::DEFAULT_BASE.into(),
            jikan: jikan::DEFAULT_BASE.into(),
            omdb: omdb::DEFAULT_BASE.into(),
            xem: xem::DEFAULT_BASE.into(),
        }
    }
}

impl Endpoints {
    /// Every catalogue at an unreachable address (tests that must not touch the network).
    pub fn offline() -> Self {
        let dead = "http://127.0.0.1:9".to_string();
        Self {
            tvmaze: dead.clone(),
            anilist: format!("{dead}/graphql"),
            tvdb: dead.clone(),
            tmdb: dead.clone(),
            jikan: dead.clone(),
            omdb: dead.clone(),
            xem: dead,
        }
    }
}

/// Provider registry with shared HTTP client, rate limits and caches.
#[derive(Clone)]
pub struct Metadata {
    pub tvmaze: tvmaze::TvMaze,
    pub anilist: anilist::AniList,
    pub tvdb: skyhook::SkyHook,
    pub tmdb: tmdb::Tmdb,
    pub jikan: jikan::Jikan,
    pub omdb: omdb::Omdb,
    pub xem: xem::Xem,
}

impl Metadata {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with(http, &Endpoints::default())
    }

    /// Uses alternative endpoints for TVmaze and AniList, the defaults for the others.
    pub fn with_endpoints(http: reqwest::Client, tvmaze_url: &str, anilist_url: &str) -> Self {
        Self::with(
            http,
            &Endpoints {
                tvmaze: tvmaze_url.into(),
                anilist: anilist_url.into(),
                ..Endpoints::default()
            },
        )
    }

    pub fn with(http: reqwest::Client, e: &Endpoints) -> Self {
        Self {
            tvmaze: tvmaze::TvMaze::with_base(http.clone(), &e.tvmaze),
            anilist: anilist::AniList::with_base(http.clone(), &e.anilist),
            tvdb: skyhook::SkyHook::with_base(http.clone(), &e.tvdb),
            tmdb: tmdb::Tmdb::with_base(http.clone(), &e.tmdb),
            jikan: jikan::Jikan::with_base(http.clone(), &e.jikan),
            omdb: omdb::Omdb::with_base(http.clone(), &e.omdb),
            xem: xem::Xem::with_base(http, &e.xem),
        }
    }

    /// The user's API keys for TMDB and OMDb (`None` or empty: the catalogue is unavailable).
    pub fn set_api_keys(&self, tmdb: Option<String>, omdb: Option<String>) {
        self.tmdb.set_api_key(tmdb);
        self.omdb.set_api_key(omdb);
    }

    /// Sonarr's preferred metadata language for TheTVDB (an ISO 639-2 code): searches and
    /// titles without their own language.
    pub fn set_tvdb_language(&self, code: &str) {
        self.tvdb.set_language(code);
    }

    pub fn supports(provider: MetadataProvider) -> bool {
        kitsunarr_domain::api::metadata::SUPPORTED_CATALOGUES.contains(&provider)
    }

    /// Whether the catalogue can be used now (it needs no key, or its key is set).
    pub fn available(&self, provider: MetadataProvider) -> bool {
        match provider {
            MetadataProvider::TmdbTv | MetadataProvider::TmdbMovie => self.tmdb.has_api_key(),
            MetadataProvider::Omdb => self.omdb.has_api_key(),
            other => Self::supports(other),
        }
    }

    /// The catalogues that can be searched, with the credit their terms ask for.
    pub fn catalogues() -> Vec<CatalogueInfo> {
        vec![
            CatalogueInfo {
                provider: MetadataProvider::Tvdb,
                name: "TheTVDB".into(),
                description: "TV series and anime, with TheTVDB's numbering (Sonarr's catalogue)"
                    .into(),
                kinds: vec![EntryKind::Series, EntryKind::Anime],
                attribution: "TV information and images are provided by TheTVDB.com, but we are not endorsed or certified by TheTVDB.com or its affiliates".into(),
                available: true,
                url: "https://thetvdb.com".into(),
            },
            CatalogueInfo {
                provider: MetadataProvider::TmdbTv,
                name: "TMDB (TV)".into(),
                description: "TV series, with TMDB's episode groups as orderings (needs an API key)"
                    .into(),
                kinds: vec![EntryKind::Series, EntryKind::Anime],
                attribution: "This product uses the TMDB API but is not endorsed or certified by TMDB".into(),
                available: true,
                url: "https://www.themoviedb.org".into(),
            },
            CatalogueInfo {
                provider: MetadataProvider::TvMaze,
                name: "TVmaze".into(),
                description: "TV series and their episode guides".into(),
                kinds: vec![EntryKind::Series],
                attribution: "TV data provided by TVmaze.com under CC BY-SA 4.0".into(),
                available: true,
                url: "https://www.tvmaze.com".into(),
            },
            CatalogueInfo {
                provider: MetadataProvider::AniList,
                name: "AniList".into(),
                description: "Anime series and anime films".into(),
                kinds: vec![EntryKind::Anime, EntryKind::Movie],
                attribution: "Anime data provided by AniList".into(),
                available: true,
                url: "https://anilist.co".into(),
            },
            CatalogueInfo {
                provider: MetadataProvider::MyAnimeList,
                name: "MyAnimeList (Jikan)".into(),
                description: "Anime with episode titles and air dates, through the Jikan API"
                    .into(),
                kinds: vec![EntryKind::Anime, EntryKind::Movie],
                attribution: "Anime data from MyAnimeList, served by the unofficial Jikan API".into(),
                available: true,
                url: "https://jikan.moe".into(),
            },
            CatalogueInfo {
                provider: MetadataProvider::TmdbMovie,
                name: "TMDB (Movies)".into(),
                description: "Films (needs an API key)".into(),
                kinds: vec![EntryKind::Movie],
                attribution: "This product uses the TMDB API but is not endorsed or certified by TMDB".into(),
                available: true,
                url: "https://www.themoviedb.org".into(),
            },
            CatalogueInfo {
                provider: MetadataProvider::Omdb,
                name: "OMDb".into(),
                description: "Films by IMDb id or title (needs an API key)".into(),
                kinds: vec![EntryKind::Movie],
                attribution: "Film data provided by the OMDb API".into(),
                available: true,
                url: "https://www.omdbapi.com".into(),
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
            MetadataProvider::Tvdb => self.tvdb.search(term).await,
            MetadataProvider::TmdbTv => self.tmdb.search_tv(term).await,
            MetadataProvider::TmdbMovie => self.tmdb.search_movies(term).await,
            MetadataProvider::MyAnimeList => self.jikan.search(term).await,
            MetadataProvider::Omdb => self.omdb.search(term).await,
        }
    }

    pub async fn series(&self, provider: MetadataProvider, id: &str) -> MetadataResult<SeriesInfo> {
        self.series_ordered(provider, id, None).await
    }

    /// A title in a given episode ordering: TheTVDB's season type (`dvd`, `absolute`, ...) or a
    /// TMDB episode group id. Other catalogues have one ordering.
    pub async fn series_ordered(
        &self,
        provider: MetadataProvider,
        id: &str,
        ordering: Option<&str>,
    ) -> MetadataResult<SeriesInfo> {
        self.series_ordered_in(provider, id, ordering, None).await
    }

    /// [`Self::series_ordered`] in a language (an ISO 639-2 code; only TheTVDB has titles in
    /// several languages).
    pub async fn series_ordered_in(
        &self,
        provider: MetadataProvider,
        id: &str,
        ordering: Option<&str>,
        language: Option<&str>,
    ) -> MetadataResult<SeriesInfo> {
        match provider {
            MetadataProvider::TvMaze => self.tvmaze.series(id).await,
            MetadataProvider::AniList => self.anilist.series(id).await,
            MetadataProvider::Tvdb => {
                self.tvdb
                    .series_in(id, ordering.unwrap_or("official"), language)
                    .await
            }
            MetadataProvider::TmdbTv => self.tmdb.tv(id, ordering).await,
            MetadataProvider::TmdbMovie => self.tmdb.movie(id).await,
            MetadataProvider::MyAnimeList => self.jikan.series(id).await,
            MetadataProvider::Omdb => self.omdb.movie(id).await,
        }
    }

    /// Like [`Self::series_ordered`] but bypasses the in-memory cache (used by refreshes).
    /// The TMDB collection a TMDB film belongs to.
    pub async fn movie_collection(&self, id: &str) -> MetadataResult<Option<MovieCollection>> {
        self.tmdb.movie_collection(id).await
    }

    /// Tanukarr's `SeriesPresentationService.Enrich`: a missing overview, poster or fanart is
    /// taken from up to two other catalogues the title has a verified id in (TheTVDB, TMDB TV,
    /// AniList, MyAnimeList), within 5 seconds each; only artwork on their own HTTPS hosts is
    /// used. Failures leave the title as it was.
    pub async fn enrich(&self, info: &mut SeriesInfo) {
        let wants = |info: &SeriesInfo| {
            let has = |t: &str| {
                info.images
                    .iter()
                    .any(|i| i.cover_type.eq_ignore_ascii_case(t))
            };
            (
                info.overview.as_deref().is_none_or(|o| o.trim().is_empty()),
                !has("poster"),
                !has("fanart"),
            )
        };
        if wants(info) == (false, false, false) || info.kind == EntryKind::Movie {
            return;
        }
        let candidates: Vec<(MetadataProvider, String)> = [
            (ExternalIdKind::Tvdb, MetadataProvider::Tvdb),
            (ExternalIdKind::Tmdb, MetadataProvider::TmdbTv),
            (ExternalIdKind::AniList, MetadataProvider::AniList),
            (ExternalIdKind::MyAnimeList, MetadataProvider::MyAnimeList),
        ]
        .into_iter()
        .filter(|(_, p)| *p != info.provider && Self::supports(*p))
        .filter_map(|(kind, p)| {
            info.external_ids
                .iter()
                .find(|e| e.kind == kind && e.value.trim().parse::<i64>().is_ok_and(|n| n > 0))
                .map(|e| (p, e.value.clone()))
        })
        .take(2)
        .collect();
        for (provider, id) in candidates {
            let (overview, poster, fanart) = wants(info);
            if !overview && !poster && !fanart {
                break;
            }
            let Ok(Ok(other)) = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.series(provider, &id),
            )
            .await
            else {
                continue;
            };
            if overview && let Some(o) = other.overview.filter(|o| !o.trim().is_empty()) {
                info.overview = Some(o);
            }
            for (wanted, kind) in [(poster, "poster"), (fanart, "fanart")] {
                if !wanted {
                    continue;
                }
                if let Some(img) = other.images.iter().find(|i| {
                    i.cover_type.eq_ignore_ascii_case(kind) && trusted_image(&i.remote_url)
                }) {
                    info.images.push(img.clone());
                }
            }
        }
    }

    /// An AniList anime with its franchise relations.
    pub async fn anime_relations(&self, id: i64) -> MetadataResult<AnimeNode> {
        anilist::relations(&self.anilist, id).await
    }

    /// A MyAnimeList anime with its franchise relations (through Jikan).
    pub async fn mal_relations(&self, id: i64) -> MetadataResult<AnimeNode> {
        self.jikan.relations(id).await
    }

    /// The episode orderings a series can use: TMDB episode groups, TheTVDB season types. Other
    /// catalogues have one order (an empty list).
    pub async fn orderings(
        &self,
        provider: MetadataProvider,
        id: &str,
    ) -> MetadataResult<Vec<EpisodeOrdering>> {
        let default = |name: &str| EpisodeOrdering {
            id: String::new(),
            name: name.into(),
            description: None,
        };
        match provider {
            MetadataProvider::TmdbTv => {
                let mut out = vec![default("Seasons (TMDB default)")];
                out.extend(self.tmdb.episode_groups(id).await?);
                Ok(out)
            }
            MetadataProvider::Tvdb => Ok(vec![
                default("Aired order (TheTVDB default)"),
                EpisodeOrdering {
                    id: "dvd".into(),
                    name: "DVD order".into(),
                    description: None,
                },
                EpisodeOrdering {
                    id: "absolute".into(),
                    name: "Absolute order".into(),
                    description: Some("One season numbered from the first episode".into()),
                },
            ]),
            _ => Ok(Vec::new()),
        }
    }

    pub async fn series_fresh(
        &self,
        provider: MetadataProvider,
        id: &str,
        ordering: Option<&str>,
    ) -> MetadataResult<SeriesInfo> {
        self.series_fresh_in(provider, id, ordering, None).await
    }

    /// [`Self::series_fresh`] in a language (TheTVDB).
    pub async fn series_fresh_in(
        &self,
        provider: MetadataProvider,
        id: &str,
        ordering: Option<&str>,
        language: Option<&str>,
    ) -> MetadataResult<SeriesInfo> {
        match provider {
            MetadataProvider::TvMaze => self.tvmaze.forget(id),
            MetadataProvider::AniList => self.anilist.forget(id),
            MetadataProvider::Tvdb => self.tvdb.forget(id),
            MetadataProvider::TmdbTv | MetadataProvider::TmdbMovie => self.tmdb.forget(id),
            MetadataProvider::MyAnimeList => self.jikan.forget(id),
            MetadataProvider::Omdb => self.omdb.forget(id),
        }
        self.series_ordered_in(provider, id, ordering, language)
            .await
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

/// Artwork hosts presentation enrichment accepts (HTTPS only).
fn trusted_image(url: &str) -> bool {
    const HOSTS: &[&str] = &[
        "s4.anilist.co",
        "s3.anilist.co",
        "s.anilist.co",
        "cdn.myanimelist.net",
        "image.tmdb.org",
        "artworks.thetvdb.com",
        "banners.thetvdb.com",
    ];
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| HOSTS.contains(&host.to_ascii_lowercase().as_str()))
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
