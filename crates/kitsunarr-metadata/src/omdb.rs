//! OMDb (`https://www.omdbapi.com/`) for films by IMDb id, as Tanukarr's `OmdbProxy`. Needs the
//! user's OMDb API key. Only `Type == "movie"` answers are accepted; a film is one feature
//! episode, as with TMDB films.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MetadataProvider, SeriesStatus, SeriesType,
};
use serde::Deserialize;

use crate::{
    EntryKind, EpisodeInfo, MetadataError, MetadataResult, RateLimiter, SearchResult, SeasonInfo,
    SeriesInfo, TtlCache, send_json,
};

const P: MetadataProvider = MetadataProvider::Omdb;
pub const DEFAULT_BASE: &str = "https://www.omdbapi.com";

#[derive(Clone)]
pub struct Omdb {
    http: reqwest::Client,
    base: String,
    key: Arc<RwLock<Option<String>>>,
    limiter: RateLimiter,
    cache: TtlCache<SeriesInfo>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Resource {
    #[serde(rename = "Title")]
    title: String,
    #[serde(rename = "Year")]
    year: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
    #[serde(rename = "Released")]
    released: Option<String>,
    #[serde(rename = "Runtime")]
    runtime: Option<String>,
    #[serde(rename = "Plot")]
    plot: Option<String>,
    #[serde(rename = "Genre")]
    genre: Option<String>,
    #[serde(rename = "Poster")]
    poster: Option<String>,
    #[serde(rename = "Response")]
    response: Option<String>,
    #[serde(rename = "Error")]
    error: Option<String>,
    #[serde(rename = "imdbID")]
    imdb_id: Option<String>,
    #[serde(rename = "imdbRating")]
    imdb_rating: Option<String>,
    #[serde(rename = "imdbVotes")]
    imdb_votes: Option<String>,
    #[serde(rename = "Search")]
    search: Vec<Resource>,
}

fn valid_imdb(id: &str) -> bool {
    id.len() >= 9 && id.starts_with("tt") && id[2..].chars().all(|c| c.is_ascii_digit())
}

/// "20 Jul 2001" → "2001-07-20".
fn released(raw: Option<&str>) -> Option<String> {
    chrono::NaiveDate::parse_from_str(raw?.trim(), "%d %b %Y")
        .ok()
        .map(|d| d.format("%Y-%m-%d").to_string())
}

fn poster(r: &Resource) -> Option<MediaImage> {
    let url = r.poster.clone().filter(|p| {
        p.starts_with("https://m.media-amazon.com/") || p.starts_with("https://ia.media-imdb.com/")
    })?;
    Some(MediaImage {
        cover_type: "poster".into(),
        remote_url: url,
    })
}

fn map(r: &Resource) -> Option<SeriesInfo> {
    let imdb = r.imdb_id.clone().filter(|i| valid_imdb(i))?;
    if !r
        .kind
        .as_deref()
        .is_some_and(|k| k.eq_ignore_ascii_case("movie"))
    {
        return None;
    }
    let release = released(r.released.as_deref());
    let year = r
        .year
        .as_deref()
        .and_then(|y| y.get(..4))
        .and_then(|y| y.parse().ok());
    let runtime = r
        .runtime
        .as_deref()
        .and_then(|t| t.split_whitespace().next())
        .and_then(|n| n.parse().ok());
    let rating = r
        .imdb_rating
        .as_deref()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.clamp(0.0, 10.0));
    let votes = r
        .imdb_votes
        .as_deref()
        .and_then(|v| v.replace(',', "").parse().ok());
    let is_released = release
        .as_deref()
        .is_some_and(|d| d <= chrono::Utc::now().format("%Y-%m-%d").to_string().as_str());
    Some(SeriesInfo {
        provider: P,
        id: imdb.clone(),
        kind: EntryKind::Movie,
        title: r.title.clone(),
        alternate_titles: Vec::new(),
        original_title: None,
        year,
        status: if is_released {
            SeriesStatus::Ended
        } else {
            SeriesStatus::Upcoming
        },
        series_type: SeriesType::Standard,
        overview: r.plot.clone().filter(|p| !p.is_empty() && p != "N/A"),
        network: None,
        air_time: None,
        runtime,
        genres: r
            .genre
            .as_deref()
            .map(|g| {
                g.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty() && x != "N/A")
                    .collect()
            })
            .unwrap_or_default(),
        images: poster(r).into_iter().collect(),
        rating,
        votes,
        first_aired: release.clone(),
        last_aired: release.clone(),
        original_language: None,
        original_country: None,
        external_ids: vec![ExternalId {
            kind: ExternalIdKind::Imdb,
            value: imdb.clone(),
        }],
        seasons: vec![SeasonInfo {
            season_number: 1,
            title: None,
        }],
        episodes: vec![EpisodeInfo {
            season_number: 1,
            episode_number: 1,
            absolute_episode_number: Some(1),
            title: Some(r.title.clone()),
            overview: None,
            air_date: release.clone(),
            air_date_utc: release.map(|d| format!("{d}T00:00:00Z")),
            runtime,
            provider_episode_id: Some(imdb),
        }],
    })
}

impl Omdb {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base(http, DEFAULT_BASE)
    }

    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            key: Arc::default(),
            // `RateLimit = 1000 ms`.
            limiter: RateLimiter::new(1, Duration::from_secs(1)),
            cache: TtlCache::new(Duration::from_secs(600), 64),
        }
    }

    pub fn set_api_key(&self, key: Option<String>) {
        *self.key.write().unwrap_or_else(|e| e.into_inner()) =
            key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    }

    pub fn has_api_key(&self) -> bool {
        self.key.read().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    async fn get(&self, query: &[(&str, &str)]) -> MetadataResult<Resource> {
        let key = self
            .key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(MetadataError::MissingKey(P))?;
        let url = format!("{}/", self.base);
        let r: Resource = send_json(P, &self.limiter, || {
            self.http
                .get(&url)
                .query(&[
                    ("apikey", key.as_str()),
                    ("type", "movie"),
                    ("plot", "full"),
                ])
                .query(query)
        })
        .await?;
        if r.response.as_deref() == Some("False") {
            let error = r.error.clone().unwrap_or_default();
            if error.to_lowercase().contains("not found") {
                return Err(MetadataError::NotFound(P));
            }
            return Err(MetadataError::Http {
                provider: P,
                message: error,
            });
        }
        Ok(r)
    }

    /// `tt…`, `imdb:tt…` and `omdb:tt…` look the film up; other terms search titles.
    pub async fn search(&self, term: &str) -> MetadataResult<Vec<SearchResult>> {
        let lower = term.trim().to_lowercase();
        let id = lower
            .trim_start_matches("imdb:")
            .trim_start_matches("omdb:")
            .trim();
        if valid_imdb(id) {
            return match self.movie(id).await {
                Ok(info) => Ok(vec![crate::result_of(&info)]),
                Err(MetadataError::NotFound(_)) => Ok(Vec::new()),
                Err(e) => Err(e),
            };
        }
        let found = match self.get(&[("s", term.trim())]).await {
            Ok(r) => r,
            Err(MetadataError::NotFound(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(found
            .search
            .iter()
            .filter_map(map)
            .map(|i| crate::result_of(&i))
            .collect())
    }

    pub fn forget(&self, id: &str) {
        self.cache.remove(id);
    }

    pub async fn movie(&self, id: &str) -> MetadataResult<SeriesInfo> {
        if let Some(hit) = self.cache.get(id) {
            return Ok(hit);
        }
        if !valid_imdb(id) {
            return Err(MetadataError::NotFound(P));
        }
        let r = self.get(&[("i", id)]).await?;
        let info = map(&r).ok_or_else(|| MetadataError::Decode {
            provider: P,
            message: format!("{id} is not a film"),
        })?;
        self.cache.insert(id.to_string(), info.clone());
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_film_and_rejects_series() {
        let r: Resource = serde_json::from_value(serde_json::json!({
            "Title": "Spirited Away", "Year": "2001", "Rated": "PG", "Released": "20 Jul 2001",
            "Runtime": "125 min", "Genre": "Animation, Adventure, Family", "Plot": "A girl…",
            "Poster": "https://m.media-amazon.com/images/M/x.jpg", "imdbRating": "8.6",
            "imdbVotes": "1,000,000", "imdbID": "tt0245429", "Type": "movie", "Response": "True"
        }))
        .unwrap();
        let info = map(&r).unwrap();
        assert_eq!(
            (info.id.as_str(), info.year, info.runtime),
            ("tt0245429", Some(2001), Some(125))
        );
        assert_eq!(info.first_aired.as_deref(), Some("2001-07-20"));
        assert_eq!(info.genres.len(), 3);
        assert_eq!(info.votes, Some(1_000_000));
        assert_eq!(info.status, SeriesStatus::Ended);
        let mut series = r.clone();
        series.kind = Some("series".into());
        assert!(map(&series).is_none());
        let mut bad_poster = r;
        bad_poster.poster = Some("http://evil.example/x.jpg".into());
        assert!(map(&bad_poster).unwrap().images.is_empty());
    }
}
