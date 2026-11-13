//! MyAnimeList through Jikan (`https://api.jikan.moe/v4`), as Tanukarr's `JikanProxy`: anime
//! with per-episode titles and air dates (which AniList lacks), no API key, and Jikan's limits
//! (3 requests a second, 60 a minute).
//!
//! Numbering follows Tanukarr so migrated titles line up: one season, episode number = Jikan's
//! episode `mal_id` (its position as fallback), absolute number = episode number, and numbers up
//! to the declared episode count filled with untitled episodes when the list is short. Unlike
//! Tanukarr, a filled-in episode has no provider id, so a later refresh only adds its title
//! (Kitsunarr matches episodes by number and never recreates them).

use std::time::Duration;

use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MetadataProvider, SeriesStatus, SeriesType,
};
use serde::Deserialize;

use crate::{
    EntryKind, EpisodeInfo, MetadataResult, RateLimiter, SearchResult, SeasonInfo, SeriesInfo,
    TtlCache, send_json,
};

const P: MetadataProvider = MetadataProvider::MyAnimeList;
pub const DEFAULT_BASE: &str = "https://api.jikan.moe/v4";
const MAX_EPISODE_PAGES: u32 = 30;
const MAX_SEARCH_RESULTS: usize = 20;

#[derive(Clone)]
pub struct Jikan {
    http: reqwest::Client,
    base: String,
    per_second: RateLimiter,
    per_minute: RateLimiter,
    cache: TtlCache<SeriesInfo>,
    relations_cache: TtlCache<RelatedAnime>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct ImageSet {
    large_image_url: Option<String>,
    image_url: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Images {
    jpg: Option<ImageSet>,
    webp: Option<ImageSet>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Aired {
    from: Option<String>,
    to: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Title {
    #[serde(rename = "type")]
    kind: String,
    title: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Anime {
    mal_id: i64,
    title: String,
    title_english: Option<String>,
    title_japanese: Option<String>,
    titles: Vec<Title>,
    #[serde(rename = "type")]
    kind: Option<String>,
    synopsis: Option<String>,
    episodes: Option<i32>,
    status: Option<String>,
    aired: Option<Aired>,
    duration: Option<String>,
    score: Option<f64>,
    scored_by: Option<i64>,
    images: Option<Images>,
    genres: Vec<Named>,
    studios: Vec<Named>,
    rating: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct One {
    data: Option<Anime>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct RelationEntry {
    mal_id: i64,
    #[serde(rename = "type")]
    kind: String,
    name: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Relation {
    relation: String,
    entry: Vec<RelationEntry>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct RelatedAnime {
    mal_id: i64,
    title: String,
    title_english: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    relations: Vec<Relation>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OneRelated {
    data: Option<RelatedAnime>,
}

/// AniList's format names for Jikan's types (`TV Special` → `TV_SPECIAL`).
fn format_of(kind: Option<&str>) -> String {
    kind.unwrap_or_default()
        .trim()
        .to_uppercase()
        .replace(' ', "_")
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Many {
    data: Vec<Anime>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Episode {
    mal_id: i32,
    title: Option<String>,
    aired: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Pagination {
    has_next_page: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Episodes {
    data: Vec<Episode>,
    pagination: Option<Pagination>,
}

fn status(raw: Option<&str>) -> SeriesStatus {
    match raw {
        Some("Finished Airing") => SeriesStatus::Ended,
        Some("Not yet aired") => SeriesStatus::Upcoming,
        _ => SeriesStatus::Continuing,
    }
}

fn kind(a: &Anime) -> EntryKind {
    if a.kind.as_deref() == Some("Movie") {
        EntryKind::Movie
    } else {
        EntryKind::Anime
    }
}

/// `ParseDuration`: "24 min per ep", "1 hr 30 min".
fn duration(raw: Option<&str>) -> Option<i32> {
    let raw = raw?;
    let words: Vec<&str> = raw.split_whitespace().collect();
    let mut minutes = 0i64;
    for (i, w) in words.iter().enumerate() {
        let Ok(v) = w.parse::<i64>() else { continue };
        match words.get(i + 1).map(|u| u.to_ascii_lowercase()) {
            Some(u) if u.starts_with("hr") => minutes += v * 60,
            Some(u) if u.starts_with("min") => minutes += v,
            _ => {}
        }
    }
    (minutes > 0).then(|| i32::try_from(minutes).unwrap_or(0))
}

fn poster(a: &Anime) -> Option<String> {
    let images = a.images.clone().unwrap_or_default();
    [
        images.jpg.as_ref().and_then(|i| i.large_image_url.clone()),
        images.jpg.as_ref().and_then(|i| i.image_url.clone()),
        images.webp.as_ref().and_then(|i| i.large_image_url.clone()),
        images.webp.as_ref().and_then(|i| i.image_url.clone()),
    ]
    .into_iter()
    .flatten()
    .find(|u| !u.trim().is_empty())
}

fn title(a: &Anime) -> String {
    a.title_english
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| a.title.clone())
}

fn date(raw: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw?)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

fn external_ids(a: &Anime) -> Vec<ExternalId> {
    vec![ExternalId {
        kind: ExternalIdKind::MyAnimeList,
        value: a.mal_id.to_string(),
    }]
}

/// `IsSuitable`: adult entries ("Rx - Hentai") are left out.
fn suitable(a: &Anime) -> bool {
    a.mal_id > 0
        && !a.title.trim().is_empty()
        && !a
            .rating
            .as_deref()
            .is_some_and(|r| r.to_ascii_lowercase().starts_with("rx"))
}

fn map_anime(a: &Anime, listed: &[Episode]) -> SeriesInfo {
    let runtime = duration(a.duration.as_deref());
    let mut episodes: Vec<EpisodeInfo> = Vec::new();
    for (i, e) in listed.iter().enumerate() {
        let number = if e.mal_id > 0 { e.mal_id } else { i as i32 + 1 };
        if episodes.iter().any(|x| x.episode_number == number) {
            continue;
        }
        let aired = date(e.aired.as_deref());
        episodes.push(EpisodeInfo {
            season_number: 1,
            episode_number: number,
            absolute_episode_number: Some(number),
            title: e.title.clone().filter(|t| !t.trim().is_empty()),
            overview: None,
            air_date: aired.map(|d| d.format("%Y-%m-%d").to_string()),
            air_date_utc: aired.map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            runtime,
            provider_episode_id: Some(number.to_string()),
        });
    }
    // `FillMissingEpisodes`: up to the declared count (at least one for a film).
    let declared = if kind(a) == EntryKind::Movie {
        a.episodes.unwrap_or(0).max(1)
    } else {
        a.episodes.unwrap_or(0)
    };
    for n in 1..=declared {
        if !episodes.iter().any(|e| e.episode_number == n) {
            episodes.push(EpisodeInfo {
                season_number: 1,
                episode_number: n,
                absolute_episode_number: Some(n),
                title: None,
                overview: None,
                air_date: None,
                air_date_utc: None,
                runtime,
                provider_episode_id: None,
            });
        }
    }
    episodes.sort_by_key(|e| e.episode_number);
    let main = title(a);
    let mut alternate_titles = Vec::new();
    for t in std::iter::once(a.title.clone())
        .chain(a.titles.iter().map(|t| t.title.clone()))
        .chain(a.title_japanese.clone())
    {
        if !t.trim().is_empty() && t != main && !alternate_titles.contains(&t) {
            alternate_titles.push(t);
        }
    }
    let from = date(a.aired.as_ref().and_then(|x| x.from.as_deref()));
    let to = date(a.aired.as_ref().and_then(|x| x.to.as_deref()));
    SeriesInfo {
        provider: P,
        id: a.mal_id.to_string(),
        kind: kind(a),
        title: main,
        alternate_titles,
        original_title: a
            .titles
            .iter()
            .find(|t| t.kind == "Japanese")
            .map(|t| t.title.clone())
            .or_else(|| a.title_japanese.clone()),
        year: from.map(|d| chrono::Datelike::year(&d)),
        status: status(a.status.as_deref()),
        series_type: SeriesType::Anime,
        overview: a.synopsis.clone().filter(|s| !s.trim().is_empty()),
        network: a
            .studios
            .first()
            .map(|s| s.name.clone())
            .filter(|n| !n.is_empty()),
        air_time: None,
        runtime,
        genres: a.genres.iter().map(|g| g.name.clone()).collect(),
        images: poster(a)
            .map(|url| MediaImage {
                cover_type: "poster".into(),
                remote_url: url,
            })
            .into_iter()
            .collect(),
        rating: a.score,
        votes: a.scored_by,
        first_aired: from.map(|d| d.format("%Y-%m-%d").to_string()),
        last_aired: to.map(|d| d.format("%Y-%m-%d").to_string()),
        original_language: kitsunarr_domain::Language::by_name("Japanese").map(|l| l.id),
        original_country: Some("JP".into()),
        external_ids: external_ids(a),
        seasons: vec![SeasonInfo {
            season_number: 1,
            title: None,
        }],
        episodes,
    }
}

impl Jikan {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base(http, DEFAULT_BASE)
    }

    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            per_second: RateLimiter::new(3, Duration::from_secs(1)),
            per_minute: RateLimiter::new(60, Duration::from_secs(60)),
            cache: TtlCache::new(Duration::from_secs(600), 64),
            relations_cache: TtlCache::new(Duration::from_secs(900), 256),
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> MetadataResult<T> {
        self.per_minute.acquire().await;
        let url = format!("{}{path}", self.base);
        send_json(P, &self.per_second, || self.http.get(&url)).await
    }

    /// `SearchForNewSeries`: `mal:{id}` looks the title up directly.
    pub async fn search(&self, term: &str) -> MetadataResult<Vec<SearchResult>> {
        let lower = term.trim().to_lowercase();
        if let Some(id) = lower
            .strip_prefix("mal:")
            .or_else(|| lower.strip_prefix("myanimelist:"))
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        {
            return match self.get::<One>(&format!("/anime/{id}/full")).await {
                Ok(One { data: Some(a) }) if suitable(&a) => Ok(vec![result_of(&a)]),
                Ok(_) | Err(crate::MetadataError::NotFound(_)) => Ok(Vec::new()),
                Err(e) => Err(e),
            };
        }
        let url = format!("{}/anime", self.base);
        self.per_minute.acquire().await;
        let found: Many = send_json(P, &self.per_second, || {
            self.http.get(&url).query(&[("q", term.trim())])
        })
        .await?;
        let mut seen = std::collections::HashSet::new();
        Ok(found
            .data
            .iter()
            .filter(|a| suitable(a) && seen.insert(a.mal_id))
            .take(MAX_SEARCH_RESULTS)
            .map(result_of)
            .collect())
    }

    pub fn forget(&self, id: &str) {
        self.cache.remove(id);
    }

    async fn related(&self, id: i64) -> MetadataResult<RelatedAnime> {
        if let Some(hit) = self.relations_cache.get(&id.to_string()) {
            return Ok(hit);
        }
        let a = self
            .get::<OneRelated>(&format!("/anime/{id}/full"))
            .await?
            .data
            .filter(|a| a.mal_id == id)
            .ok_or(crate::MetadataError::NotFound(P))?;
        self.relations_cache.insert(id.to_string(), a.clone());
        Ok(a)
    }

    /// `JikanProxy.GetAnimeRelations`: prequels, sequels, parent and side stories that are
    /// anime. Jikan does not give the neighbours' types, so each costs a request of its own
    /// (cached for 15 minutes).
    pub async fn relations(&self, id: i64) -> MetadataResult<crate::AnimeNode> {
        let a = self.related(id).await?;
        let mut relations = Vec::new();
        for r in &a.relations {
            let kind = match r.relation.as_str() {
                "Prequel" => "PREQUEL",
                "Sequel" => "SEQUEL",
                "Parent story" => "PARENT",
                "Side story" => "SIDE_STORY",
                _ => continue,
            };
            for e in r.entry.iter().filter(|e| e.kind == "anime" && e.mal_id > 0) {
                let format = match self.related(e.mal_id).await {
                    Ok(n) => format_of(n.kind.as_deref()),
                    Err(crate::MetadataError::NotFound(_)) => String::new(),
                    Err(err) => return Err(err),
                };
                relations.push(crate::AnimeRelation {
                    kind: kind.into(),
                    id: e.mal_id,
                    format,
                    title: e.name.clone(),
                });
            }
        }
        Ok(crate::AnimeNode {
            id: a.mal_id,
            format: format_of(a.kind.as_deref()),
            title: a
                .title_english
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| a.title.clone()),
            relations,
        })
    }

    pub async fn series(&self, id: &str) -> MetadataResult<SeriesInfo> {
        if let Some(hit) = self.cache.get(id) {
            return Ok(hit);
        }
        let mal_id: i64 = id
            .trim()
            .parse()
            .ok()
            .filter(|v| *v > 0)
            .ok_or(crate::MetadataError::NotFound(P))?;
        let anime = self
            .get::<One>(&format!("/anime/{mal_id}/full"))
            .await?
            .data
            .filter(|a| a.mal_id == mal_id && !a.title.trim().is_empty())
            .ok_or(crate::MetadataError::NotFound(P))?;
        // The episode list comes from MyAnimeList itself and fails more often than the series
        // call: what was read is kept and the declared count fills the rest, as upstream.
        let mut listed = Vec::new();
        for page in 1..=MAX_EPISODE_PAGES {
            match self
                .get::<Episodes>(&format!("/anime/{mal_id}/episodes?page={page}"))
                .await
            {
                Ok(e) => {
                    let more = e.pagination.as_ref().is_some_and(|p| p.has_next_page);
                    let empty = e.data.is_empty();
                    listed.extend(e.data);
                    if empty || !more {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(mal_id, error = %e, "could not read the episode list; using the declared count");
                    break;
                }
            }
        }
        let info = map_anime(&anime, &listed);
        self.cache.insert(id.to_string(), info.clone());
        Ok(info)
    }
}

fn result_of(a: &Anime) -> SearchResult {
    SearchResult {
        provider: P,
        id: a.mal_id.to_string(),
        kind: kind(a),
        title: title(a),
        year: date(a.aired.as_ref().and_then(|x| x.from.as_deref()))
            .map(|d| chrono::Datelike::year(&d)),
        overview: a.synopsis.clone().filter(|s| !s.trim().is_empty()),
        network: a.studios.first().map(|s| s.name.clone()),
        status: status(a.status.as_deref()),
        poster_url: poster(a),
        external_ids: external_ids(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Jikan v4's documented shapes (`/anime/{id}/full`, `/anime/{id}/episodes`).
    fn bebop() -> Anime {
        serde_json::from_value(serde_json::json!({
            "mal_id": 1, "title": "Cowboy Bebop", "title_english": "Cowboy Bebop",
            "title_japanese": "カウボーイビバップ",
            "titles": [{"type": "Default", "title": "Cowboy Bebop"}, {"type": "Japanese", "title": "カウボーイビバップ"}],
            "type": "TV", "episodes": 26, "status": "Finished Airing",
            "aired": {"from": "1998-04-03T00:00:00+00:00", "to": "1999-04-24T00:00:00+00:00"},
            "duration": "24 min per ep", "rating": "R - 17+ (violence & profanity)",
            "score": 8.75, "scored_by": 1000000,
            "images": {"jpg": {"image_url": "https://cdn.myanimelist.net/images/anime/4/19644.jpg",
                               "large_image_url": "https://cdn.myanimelist.net/images/anime/4/19644l.jpg"}},
            "genres": [{"mal_id": 1, "type": "anime", "name": "Action"}],
            "studios": [{"mal_id": 14, "type": "anime", "name": "Sunrise"}],
            "synopsis": "Crime is timeless."
        }))
        .unwrap()
    }

    #[test]
    fn maps_series_episodes_and_fills_the_declared_count() {
        let episodes: Episodes = serde_json::from_value(serde_json::json!({
            "pagination": {"last_visible_page": 1, "has_next_page": false},
            "data": [
                {"mal_id": 1, "title": "Asteroid Blues", "aired": "1998-10-24T00:00:00+00:00", "filler": false, "recap": false},
                {"mal_id": 2, "title": "Stray Dog Strut", "aired": "1998-10-31T00:00:00+00:00"}
            ]
        }))
        .unwrap();
        let info = map_anime(&bebop(), &episodes.data);
        assert_eq!(info.title, "Cowboy Bebop");
        assert_eq!(
            (info.kind, info.series_type),
            (EntryKind::Anime, SeriesType::Anime)
        );
        assert_eq!((info.year, info.status), (Some(1998), SeriesStatus::Ended));
        assert_eq!(info.runtime, Some(24));
        assert_eq!(info.network.as_deref(), Some("Sunrise"));
        assert_eq!(info.episodes.len(), 26);
        let first = &info.episodes[0];
        assert_eq!(
            (
                first.season_number,
                first.episode_number,
                first.absolute_episode_number
            ),
            (1, 1, Some(1))
        );
        assert_eq!(first.title.as_deref(), Some("Asteroid Blues"));
        assert_eq!(first.air_date.as_deref(), Some("1998-10-24"));
        assert_eq!(info.episodes[25].title, None);
        assert_eq!(info.episodes[25].provider_episode_id, None);
        assert_eq!(info.original_title.as_deref(), Some("カウボーイビバップ"));
        assert!(info.images[0].remote_url.ends_with("19644l.jpg"));
    }

    #[test]
    fn films_have_one_episode_and_adult_entries_are_dropped() {
        let mut film = bebop();
        film.kind = Some("Movie".into());
        film.episodes = None;
        let info = map_anime(&film, &[]);
        assert_eq!(info.kind, EntryKind::Movie);
        assert_eq!(info.episodes.len(), 1);
        let mut adult = bebop();
        adult.rating = Some("Rx - Hentai".into());
        assert!(!suitable(&adult));
        assert!(suitable(&bebop()));
    }

    #[test]
    fn durations_and_statuses() {
        assert_eq!(duration(Some("24 min per ep")), Some(24));
        assert_eq!(duration(Some("1 hr 30 min")), Some(90));
        assert_eq!(duration(Some("Unknown")), None);
        assert_eq!(status(Some("Currently Airing")), SeriesStatus::Continuing);
        assert_eq!(status(Some("Not yet aired")), SeriesStatus::Upcoming);
    }
}
