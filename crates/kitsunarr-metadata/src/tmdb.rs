//! TMDB (`https://api.themoviedb.org/3`) for TV series and films, as Tanukarr's `TmdbProxy` and
//! `TmdbMovieProxy`. Needs the user's TMDB v3 API key (Settings › Metadata), sent as `api_key`.
//!
//! TV: `GET /tv/{id}?append_to_response=external_ids,content_ratings`, then one
//! `GET /tv/{id}/season/{n}` per season; episode groups (`GET /tv/episode_group/{groupId}`)
//! renumber the episodes when the title has an ordering. Absolute numbers are given to every
//! regular episode in (season, episode) order, after the ordering, as upstream does.
//! Films: `GET /movie/{id}`; `tt…` search terms go through `GET /find/{imdbId}`.

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

pub const DEFAULT_BASE: &str = "https://api.themoviedb.org/3";
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

#[derive(Clone)]
pub struct Tmdb {
    http: reqwest::Client,
    base: String,
    key: Arc<RwLock<Option<String>>>,
    limiter: RateLimiter,
    cache: TtlCache<SeriesInfo>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct SeasonRef {
    season_number: i32,
    name: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct ExternalIds {
    imdb_id: Option<String>,
    tvdb_id: Option<i64>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Show {
    id: i64,
    name: String,
    original_name: Option<String>,
    overview: Option<String>,
    first_air_date: Option<String>,
    last_air_date: Option<String>,
    status: Option<String>,
    episode_run_time: Vec<i32>,
    original_language: Option<String>,
    genres: Vec<Named>,
    vote_average: Option<f64>,
    vote_count: Option<i64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    networks: Vec<Named>,
    seasons: Vec<SeasonRef>,
    external_ids: Option<ExternalIds>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Episode {
    id: i64,
    episode_number: i32,
    season_number: i32,
    name: Option<String>,
    overview: Option<String>,
    air_date: Option<String>,
    runtime: Option<i32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SeasonDetail {
    episodes: Vec<Episode>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct GroupEntry {
    id: i64,
    order: i32,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Group {
    order: i32,
    episodes: Vec<GroupEntry>,
}

/// `TmdbEpisodeGroupDetailResource`.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct EpisodeGroup {
    groups: Vec<Group>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Movie {
    id: i64,
    title: String,
    original_title: Option<String>,
    original_language: Option<String>,
    overview: Option<String>,
    status: Option<String>,
    runtime: Option<i32>,
    genres: Vec<Named>,
    release_date: Option<String>,
    imdb_id: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    vote_average: Option<f64>,
    vote_count: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Results<T> {
    results: Vec<T>,
    movie_results: Vec<T>,
}

/// `MapStatus`.
fn tv_status(raw: Option<&str>) -> SeriesStatus {
    match raw {
        Some("Ended") | Some("Canceled") => SeriesStatus::Ended,
        Some("In Production") | Some("Planned") | Some("Pilot") => SeriesStatus::Upcoming,
        _ => SeriesStatus::Continuing,
    }
}

fn year(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

fn language(code: Option<&str>) -> Option<i32> {
    code.and_then(|c| kitsunarr_parser::find_iso(&c.to_lowercase()))
        .map(|l| l.language().id)
        .or(Some(1))
}

fn image(path: Option<&str>, cover_type: &str) -> Option<MediaImage> {
    let path = path.filter(|p| p.starts_with('/'))?;
    Some(MediaImage {
        cover_type: cover_type.into(),
        remote_url: format!("{IMAGE_BASE}/original{path}"),
    })
}

fn is_anime(show: &Show) -> bool {
    show.original_language.as_deref() == Some("ja")
        && show.genres.iter().any(|g| g.name == "Animation")
}

fn show_ids(show: &Show) -> Vec<ExternalId> {
    let mut ids = vec![ExternalId {
        kind: ExternalIdKind::Tmdb,
        value: show.id.to_string(),
    }];
    if let Some(ext) = &show.external_ids {
        if let Some(tvdb) = ext.tvdb_id.filter(|v| *v > 0) {
            ids.push(ExternalId {
                kind: ExternalIdKind::Tvdb,
                value: tvdb.to_string(),
            });
        }
        if let Some(imdb) = ext.imdb_id.clone().filter(|v| v.starts_with("tt")) {
            ids.push(ExternalId {
                kind: ExternalIdKind::Imdb,
                value: imdb,
            });
        }
    }
    ids
}

/// `TmdbEpisodeGroupProjector.Project`: within the group, an entry's position gives its
/// episode number and its group's order the season; episodes the group does not mention keep
/// their numbering, so nothing is dropped.
pub(crate) fn project(group: &EpisodeGroup, episodes: Vec<EpisodeInfo>) -> Vec<EpisodeInfo> {
    let mut by_id: std::collections::HashMap<String, EpisodeInfo> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in episodes {
        if let Some(id) = e.provider_episode_id.clone() {
            order.push(id.clone());
            by_id.insert(id, e);
        }
    }
    let mut projected = Vec::new();
    let mut claimed = std::collections::HashSet::new();
    let mut groups = group.groups.clone();
    groups.sort_by_key(|g| g.order);
    for g in groups {
        let mut entries = g.episodes.clone();
        entries.sort_by_key(|e| e.order);
        let mut number = 1;
        for entry in entries {
            let id = entry.id.to_string();
            if !claimed.insert(id.clone()) {
                continue;
            }
            if let Some(mut e) = by_id.get(&id).cloned() {
                e.season_number = g.order;
                e.episode_number = number;
                number += 1;
                projected.push(e);
            }
        }
    }
    for id in order {
        if !claimed.contains(&id)
            && let Some(e) = by_id.get(&id)
        {
            projected.push(e.clone());
        }
    }
    projected
}

/// `SetAbsoluteNumbering`.
fn number_absolutely(episodes: &mut [EpisodeInfo]) {
    let mut regular: Vec<&mut EpisodeInfo> = episodes
        .iter_mut()
        .filter(|e| e.season_number > 0)
        .collect();
    regular.sort_by_key(|e| (e.season_number, e.episode_number));
    for (i, e) in regular.into_iter().enumerate() {
        e.absolute_episode_number = Some(i as i32 + 1);
    }
}

fn map_show(
    show: &Show,
    mut episodes: Vec<EpisodeInfo>,
    group: Option<&EpisodeGroup>,
) -> SeriesInfo {
    if let Some(g) = group {
        episodes = project(g, episodes);
    }
    number_absolutely(&mut episodes);
    episodes.sort_by_key(|e| (e.season_number, e.episode_number));
    // `SetSeasons`: the seasons the episodes stand in, after any ordering.
    let mut seasons: Vec<SeasonInfo> = Vec::new();
    for e in &episodes {
        if !seasons.iter().any(|s| s.season_number == e.season_number) {
            seasons.push(SeasonInfo {
                season_number: e.season_number,
                title: if group.is_none() {
                    show.seasons
                        .iter()
                        .find(|s| s.season_number == e.season_number)
                        .and_then(|s| s.name.clone())
                } else {
                    None
                },
            });
        }
    }
    let anime = is_anime(show);
    SeriesInfo {
        provider: MetadataProvider::TmdbTv,
        id: show.id.to_string(),
        kind: if anime {
            EntryKind::Anime
        } else {
            EntryKind::Series
        },
        title: show.name.clone(),
        alternate_titles: show
            .original_name
            .clone()
            .filter(|o| !o.is_empty() && *o != show.name)
            .into_iter()
            .collect(),
        original_title: show.original_name.clone().filter(|o| !o.is_empty()),
        year: year(show.first_air_date.as_deref()),
        status: tv_status(show.status.as_deref()),
        series_type: if anime {
            SeriesType::Anime
        } else {
            SeriesType::Standard
        },
        overview: show.overview.clone().filter(|o| !o.is_empty()),
        network: show.networks.first().map(|n| n.name.clone()),
        air_time: None,
        runtime: show.episode_run_time.first().copied(),
        genres: show.genres.iter().map(|g| g.name.clone()).collect(),
        images: [
            image(show.poster_path.as_deref(), "poster"),
            image(show.backdrop_path.as_deref(), "fanart"),
        ]
        .into_iter()
        .flatten()
        .collect(),
        rating: show.vote_average,
        votes: show.vote_count,
        first_aired: show.first_air_date.clone().filter(|d| !d.is_empty()),
        last_aired: show.last_air_date.clone().filter(|d| !d.is_empty()),
        original_language: language(show.original_language.as_deref()),
        external_ids: show_ids(show),
        seasons,
        episodes,
    }
}

fn map_episode(e: &Episode) -> EpisodeInfo {
    let air = e.air_date.clone().filter(|d| d.len() == 10);
    EpisodeInfo {
        season_number: e.season_number,
        episode_number: e.episode_number,
        absolute_episode_number: None,
        title: e.name.clone().filter(|t| !t.is_empty()),
        overview: e.overview.clone().filter(|t| !t.is_empty()),
        air_date_utc: air.as_ref().map(|d| format!("{d}T00:00:00Z")),
        air_date: air,
        runtime: e.runtime,
        provider_episode_id: Some(e.id.to_string()),
    }
}

/// `MovieMetadata.Create` + `WithFeature`: one season, one episode (the feature).
fn map_movie(m: &Movie) -> SeriesInfo {
    let released = m.status.as_deref() == Some("Released")
        || m.release_date
            .as_deref()
            .is_some_and(|d| d < chrono::Utc::now().format("%Y-%m-%d").to_string().as_str());
    let mut ids = vec![ExternalId {
        kind: ExternalIdKind::Tmdb,
        value: m.id.to_string(),
    }];
    if let Some(imdb) = m.imdb_id.clone().filter(|v| v.starts_with("tt")) {
        ids.push(ExternalId {
            kind: ExternalIdKind::Imdb,
            value: imdb,
        });
    }
    let release = m.release_date.clone().filter(|d| !d.is_empty());
    SeriesInfo {
        provider: MetadataProvider::TmdbMovie,
        id: m.id.to_string(),
        kind: EntryKind::Movie,
        title: m.title.clone(),
        alternate_titles: m
            .original_title
            .clone()
            .filter(|o| !o.is_empty() && *o != m.title)
            .into_iter()
            .collect(),
        original_title: m.original_title.clone().filter(|o| !o.is_empty()),
        year: year(release.as_deref()),
        status: if released {
            SeriesStatus::Ended
        } else {
            SeriesStatus::Upcoming
        },
        series_type: SeriesType::Standard,
        overview: m.overview.clone().filter(|o| !o.is_empty()),
        network: None,
        air_time: None,
        runtime: m.runtime.filter(|r| *r > 0),
        genres: m.genres.iter().map(|g| g.name.clone()).collect(),
        images: [
            image(m.poster_path.as_deref(), "poster"),
            image(m.backdrop_path.as_deref(), "fanart"),
        ]
        .into_iter()
        .flatten()
        .collect(),
        rating: m.vote_average,
        votes: m.vote_count,
        first_aired: release.clone(),
        last_aired: release.clone(),
        original_language: language(m.original_language.as_deref()),
        external_ids: ids,
        seasons: vec![SeasonInfo {
            season_number: 1,
            title: None,
        }],
        episodes: vec![EpisodeInfo {
            season_number: 1,
            episode_number: 1,
            absolute_episode_number: Some(1),
            title: Some(m.title.clone()),
            overview: None,
            air_date: release.clone(),
            air_date_utc: release.map(|d| format!("{d}T00:00:00Z")),
            runtime: m.runtime.filter(|r| *r > 0),
            provider_episode_id: Some(m.id.to_string()),
        }],
    }
}

impl Tmdb {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base(http, DEFAULT_BASE)
    }

    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            key: Arc::default(),
            // `RateLimit = 250 ms`.
            limiter: RateLimiter::new(4, Duration::from_secs(1)),
            cache: TtlCache::new(Duration::from_secs(600), 64),
        }
    }

    /// The user's TMDB v3 API key (`None` disables TMDB).
    pub fn set_api_key(&self, key: Option<String>) {
        *self.key.write().unwrap_or_else(|e| e.into_inner()) =
            key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    }

    pub fn has_api_key(&self) -> bool {
        self.key.read().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        provider: MetadataProvider,
        path: &str,
        query: &[(&str, &str)],
    ) -> MetadataResult<T> {
        let key = self
            .key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(MetadataError::MissingKey(provider))?;
        let url = format!("{}{path}", self.base);
        let result = send_json(provider, &self.limiter, || {
            self.http
                .get(&url)
                .query(&[("api_key", key.as_str())])
                .query(query)
        })
        .await;
        match result {
            Err(MetadataError::Http { message, .. }) if message.contains("401") => {
                Err(MetadataError::Http {
                    provider,
                    message: "the TMDB API key was rejected".into(),
                })
            }
            other => other,
        }
    }

    pub async fn search_tv(&self, term: &str) -> MetadataResult<Vec<SearchResult>> {
        let lower = term.trim().to_lowercase();
        if let Some(id) = lower
            .strip_prefix("tmdb:")
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        {
            return match self.tv(id, None).await {
                Ok(info) => Ok(vec![crate::result_of(&info)]),
                Err(MetadataError::NotFound(_)) => Ok(Vec::new()),
                Err(e) => Err(e),
            };
        }
        let found: Results<Show> = self
            .get(
                MetadataProvider::TmdbTv,
                "/search/tv",
                &[("query", term.trim())],
            )
            .await?;
        Ok(found
            .results
            .iter()
            .filter(|s| s.id > 0)
            .map(|s| SearchResult {
                provider: MetadataProvider::TmdbTv,
                id: s.id.to_string(),
                kind: if is_anime(s) {
                    EntryKind::Anime
                } else {
                    EntryKind::Series
                },
                title: s.name.clone(),
                year: year(s.first_air_date.as_deref()),
                overview: s.overview.clone().filter(|o| !o.is_empty()),
                network: None,
                status: tv_status(s.status.as_deref()),
                poster_url: image(s.poster_path.as_deref(), "poster").map(|i| i.remote_url),
                external_ids: show_ids(s),
            })
            .collect())
    }

    pub async fn search_movies(&self, term: &str) -> MetadataResult<Vec<SearchResult>> {
        let trimmed = term.trim();
        let lower = trimmed.to_lowercase();
        let movies: Vec<Movie> = if let Some(id) = lower
            .strip_prefix("tmdb:")
            .or_else(|| lower.strip_prefix("tmdbmovie:"))
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        {
            match self
                .get::<Movie>(MetadataProvider::TmdbMovie, &format!("/movie/{id}"), &[])
                .await
            {
                Ok(m) => vec![m],
                Err(MetadataError::NotFound(_)) => Vec::new(),
                Err(e) => return Err(e),
            }
        } else if let Some(imdb) = Some(lower.trim_start_matches("imdb:").trim())
            .filter(|t| t.starts_with("tt") && t[2..].chars().all(|c| c.is_ascii_digit()))
        {
            self.get::<Results<Movie>>(
                MetadataProvider::TmdbMovie,
                &format!("/find/{imdb}"),
                &[("external_source", "imdb_id")],
            )
            .await?
            .movie_results
        } else {
            self.get::<Results<Movie>>(
                MetadataProvider::TmdbMovie,
                "/search/movie",
                &[("query", trimmed)],
            )
            .await?
            .results
        };
        Ok(movies
            .iter()
            .filter(|m| m.id > 0)
            .map(|m| crate::result_of(&map_movie(m)))
            .collect())
    }

    pub fn forget(&self, id: &str) {
        self.cache.remove(&format!("tv/{id}"));
        self.cache.remove(&format!("movie/{id}"));
        self.cache.remove_prefix(&format!("tv/{id}/"));
    }

    /// A TV series, renumbered by an episode group when `ordering` names one.
    pub async fn tv(&self, id: &str, ordering: Option<&str>) -> MetadataResult<SeriesInfo> {
        let ordering = ordering.map(str::trim).filter(|o| !o.is_empty());
        let key = match ordering {
            Some(o) => format!("tv/{id}/{o}"),
            None => format!("tv/{id}"),
        };
        if let Some(hit) = self.cache.get(&key) {
            return Ok(hit);
        }
        let tmdb_id: i64 = id
            .trim()
            .parse()
            .ok()
            .filter(|v| *v > 0)
            .ok_or(MetadataError::NotFound(MetadataProvider::TmdbTv))?;
        let p = MetadataProvider::TmdbTv;
        let show: Show = self
            .get(
                p,
                &format!("/tv/{tmdb_id}"),
                &[("append_to_response", "external_ids,content_ratings")],
            )
            .await?;
        let mut episodes = Vec::new();
        let mut seasons = show.seasons.clone();
        seasons.sort_by_key(|s| s.season_number);
        for s in seasons {
            let detail: SeasonDetail = self
                .get(p, &format!("/tv/{tmdb_id}/season/{}", s.season_number), &[])
                .await?;
            episodes.extend(detail.episodes.iter().map(map_episode));
        }
        // A failed ordering fails the refresh: renumbering against half an ordering would
        // scramble the library, so the numbering on disk is kept.
        let group = match ordering {
            Some(o) => Some(
                self.get::<EpisodeGroup>(p, &format!("/tv/episode_group/{o}"), &[])
                    .await?,
            ),
            None => None,
        };
        let info = map_show(&show, episodes, group.as_ref());
        self.cache.insert(key, info.clone());
        Ok(info)
    }

    /// A series' episode groups (`GET /tv/{id}/episode_groups`), for choosing an ordering.
    pub async fn episode_groups(&self, id: &str) -> MetadataResult<Vec<crate::EpisodeOrdering>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Groups {
            results: Vec<GroupSummary>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct GroupSummary {
            id: String,
            name: String,
            description: Option<String>,
            r#type: i64,
            episode_count: Option<i64>,
            group_count: Option<i64>,
        }
        let tmdb_id: i64 = id
            .trim()
            .parse()
            .ok()
            .filter(|v| *v > 0)
            .ok_or(MetadataError::NotFound(MetadataProvider::TmdbTv))?;
        let groups: Groups = self
            .get(
                MetadataProvider::TmdbTv,
                &format!("/tv/{tmdb_id}/episode_groups"),
                &[],
            )
            .await?;
        let kind = |t: i64| match t {
            1 => "Original air date",
            2 => "Absolute",
            3 => "DVD",
            4 => "Digital",
            5 => "Story arc",
            6 => "Production",
            7 => "TV",
            _ => "Episode group",
        };
        Ok(groups
            .results
            .into_iter()
            .filter(|g| !g.id.is_empty())
            .map(|g| crate::EpisodeOrdering {
                name: format!("{} ({})", g.name, kind(g.r#type)),
                description: g.description.filter(|d| !d.trim().is_empty()).or_else(|| {
                    Some(format!(
                        "{} episodes in {} groups",
                        g.episode_count.unwrap_or(0),
                        g.group_count.unwrap_or(0)
                    ))
                }),
                id: g.id,
            })
            .collect())
    }

    pub async fn movie(&self, id: &str) -> MetadataResult<SeriesInfo> {
        let key = format!("movie/{id}");
        if let Some(hit) = self.cache.get(&key) {
            return Ok(hit);
        }
        let tmdb_id: i64 = id
            .trim()
            .parse()
            .ok()
            .filter(|v| *v > 0)
            .ok_or(MetadataError::NotFound(MetadataProvider::TmdbMovie))?;
        let movie: Movie = self
            .get(
                MetadataProvider::TmdbMovie,
                &format!("/movie/{tmdb_id}"),
                &[],
            )
            .await?;
        if movie.id != tmdb_id {
            return Err(MetadataError::Decode {
                provider: MetadataProvider::TmdbMovie,
                message: format!("asked for {tmdb_id}, got {}", movie.id),
            });
        }
        let info = map_movie(&movie);
        self.cache.insert(key, info.clone());
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(id: i64, season: i32, number: i32) -> EpisodeInfo {
        map_episode(&Episode {
            id,
            season_number: season,
            episode_number: number,
            name: Some(format!("E{id}")),
            air_date: Some("2020-01-01".into()),
            ..Default::default()
        })
    }

    /// TMDB's documented `/tv/{id}` shape.
    fn show() -> Show {
        serde_json::from_value(serde_json::json!({
            "id": 37854, "name": "One Piece", "original_name": "ワンピース",
            "first_air_date": "1999-10-20", "last_air_date": "2026-11-01", "status": "Returning Series",
            "episode_run_time": [24], "original_language": "ja",
            "genres": [{"id": 16, "name": "Animation"}], "vote_average": 8.7, "vote_count": 4000,
            "poster_path": "/poster.jpg", "backdrop_path": "/backdrop.jpg",
            "networks": [{"id": 1, "name": "Fuji TV"}],
            "seasons": [{"season_number": 0, "name": "Specials"}, {"season_number": 1, "name": "East Blue"}],
            "external_ids": {"imdb_id": "tt0388629", "tvdb_id": 81797}
        }))
        .unwrap()
    }

    #[test]
    fn maps_a_series_with_absolute_numbers() {
        let info = map_show(&show(), vec![ep(10, 0, 1), ep(2, 1, 2), ep(1, 1, 1)], None);
        assert_eq!(info.title, "One Piece");
        assert_eq!(info.kind, EntryKind::Anime);
        assert_eq!(info.status, SeriesStatus::Continuing);
        assert_eq!(info.network.as_deref(), Some("Fuji TV"));
        assert_eq!(info.runtime, Some(24));
        assert_eq!(
            info.original_language,
            kitsunarr_domain::Language::by_name("Japanese").map(|l| l.id)
        );
        assert_eq!(
            info.images[0].remote_url,
            "https://image.tmdb.org/t/p/original/poster.jpg"
        );
        let abs: Vec<(i32, i32, Option<i32>)> = info
            .episodes
            .iter()
            .map(|e| (e.season_number, e.episode_number, e.absolute_episode_number))
            .collect();
        assert_eq!(abs, vec![(0, 1, None), (1, 1, Some(1)), (1, 2, Some(2))]);
        assert_eq!(info.seasons[1].title.as_deref(), Some("East Blue"));
        assert!(
            info.external_ids
                .iter()
                .any(|i| i.kind == ExternalIdKind::Tvdb && i.value == "81797")
        );
        assert_eq!(
            info.episodes[1].air_date_utc.as_deref(),
            Some("2020-01-01T00:00:00Z")
        );
    }

    /// `TmdbEpisodeGroupProjectorFixture`: positions number the episodes, group order the
    /// seasons; duplicates are ignored and unlisted episodes keep their numbers.
    #[test]
    fn episode_groups_renumber_without_dropping_episodes() {
        let group: EpisodeGroup = serde_json::from_value(serde_json::json!({
            "groups": [
                {"order": 2, "episodes": [{"id": 3, "order": 0}]},
                {"order": 1, "episodes": [{"id": 2, "order": 1}, {"id": 1, "order": 0}, {"id": 1, "order": 2}]}
            ]
        }))
        .unwrap();
        let projected = project(
            &group,
            vec![ep(1, 1, 1), ep(2, 1, 2), ep(3, 2, 1), ep(4, 0, 1)],
        );
        let numbers: Vec<(String, i32, i32)> = projected
            .iter()
            .map(|e| {
                (
                    e.provider_episode_id.clone().unwrap(),
                    e.season_number,
                    e.episode_number,
                )
            })
            .collect();
        assert_eq!(
            numbers,
            vec![
                ("1".into(), 1, 1),
                ("2".into(), 1, 2),
                ("3".into(), 2, 1),
                ("4".into(), 0, 1)
            ]
        );
        // An absolute group puts everything in one season; seasons follow the episodes.
        let absolute: EpisodeGroup = serde_json::from_value(serde_json::json!({
            "groups": [{"order": 1, "episodes": [{"id": 1, "order": 0}, {"id": 2, "order": 1}, {"id": 3, "order": 2}]}]
        }))
        .unwrap();
        let info = map_show(
            &show(),
            vec![ep(1, 1, 1), ep(2, 1, 2), ep(3, 2, 1)],
            Some(&absolute),
        );
        assert_eq!(info.seasons.len(), 1);
        assert_eq!(info.episodes[2].episode_number, 3);
        assert_eq!(info.episodes[2].absolute_episode_number, Some(3));
    }

    #[test]
    fn films_are_one_feature() {
        let m: Movie = serde_json::from_value(serde_json::json!({
            "id": 129, "title": "Spirited Away", "original_title": "千と千尋の神隠し",
            "original_language": "ja", "status": "Released", "runtime": 125,
            "release_date": "2001-07-20", "imdb_id": "tt0245429",
            "genres": [{"id": 16, "name": "Animation"}], "poster_path": "/p.jpg"
        }))
        .unwrap();
        let info = map_movie(&m);
        assert_eq!(
            (info.kind, info.provider),
            (EntryKind::Movie, MetadataProvider::TmdbMovie)
        );
        assert_eq!((info.year, info.status), (Some(2001), SeriesStatus::Ended));
        assert_eq!(info.episodes.len(), 1);
        assert_eq!(info.alternate_titles, vec!["千と千尋の神隠し".to_string()]);
        assert!(
            info.external_ids
                .iter()
                .any(|i| i.kind == ExternalIdKind::Imdb)
        );
    }

    #[test]
    fn statuses_follow_upstream() {
        assert_eq!(tv_status(Some("Canceled")), SeriesStatus::Ended);
        assert_eq!(tv_status(Some("Planned")), SeriesStatus::Upcoming);
        assert_eq!(
            tv_status(Some("Returning Series")),
            SeriesStatus::Continuing
        );
    }

    #[tokio::test]
    async fn a_missing_key_is_reported() {
        let t = Tmdb::new(reqwest::Client::new());
        assert!(matches!(
            t.search_tv("x").await,
            Err(MetadataError::MissingKey(MetadataProvider::TmdbTv))
        ));
        t.set_api_key(Some("  ".into()));
        assert!(!t.has_api_key());
    }
}
