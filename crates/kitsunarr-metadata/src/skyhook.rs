//! TheTVDB through SkyHook (`https://skyhook.sonarr.tv/v2/tvdb/`), the proxy Sonarr and
//! Tanukarr use: no API key, TheTVDB's numbering and artwork. A port of Sonarr's
//! `SkyHookProxy` mapping (`MapSeries`, `MapEpisode`, `MapSeriesStatus`).
//!
//! Requests: `GET {base}/shows/{iso639-3}/{tvdbId}/{seasonType}` and
//! `GET {base}/search/?language={iso639-3}&term={term}`. Titles come in the series' metadata
//! language, else the preferred one (Sonarr's `PreferredMetadataLanguage`, English by default);
//! the season type is the title's episode ordering (`official`, `dvd`, `absolute`, ...).

use std::time::Duration;

use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MetadataProvider, SeriesStatus, SeriesType,
};
use serde::Deserialize;

use crate::{
    EntryKind, EpisodeInfo, MetadataResult, RateLimiter, SearchResult, SeasonInfo, SeriesInfo,
    TtlCache, send_json,
};

const P: MetadataProvider = MetadataProvider::Tvdb;
pub const DEFAULT_BASE: &str = "https://skyhook.sonarr.tv/v2/tvdb";
const DEFAULT_LANGUAGE: &str = "eng";

#[derive(Clone)]
pub struct SkyHook {
    http: reqwest::Client,
    base: String,
    limiter: RateLimiter,
    /// Answers with the language they were asked in.
    cache: TtlCache<(String, SeriesInfo)>,
    /// The preferred language's ISO 639-2 code, for searches and series without one.
    language: std::sync::Arc<std::sync::RwLock<String>>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct TimeOfDay {
    hours: Option<i32>,
    minutes: Option<i32>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Rating {
    count: Option<i64>,
    value: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Image {
    cover_type: String,
    url: String,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Translation {
    language: String,
    title: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct AlternativeTitle {
    title: String,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Season {
    season_number: i32,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Episode {
    tvdb_id: i64,
    season_number: i32,
    episode_number: i32,
    absolute_episode_number: Option<i32>,
    title: Option<String>,
    air_date: Option<String>,
    air_date_utc: Option<String>,
    runtime: Option<i32>,
    overview: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Show {
    tvdb_id: i64,
    title: String,
    tmdb_id: Option<i64>,
    tv_rage_id: Option<i64>,
    tv_maze_id: Option<i64>,
    imdb_id: Option<String>,
    overview: Option<String>,
    original_language: Option<String>,
    first_aired: Option<String>,
    last_aired: Option<String>,
    mal_ids: Vec<i64>,
    ani_list_ids: Vec<i64>,
    status: Option<String>,
    runtime: Option<i32>,
    time_of_day: Option<TimeOfDay>,
    network: Option<String>,
    genres: Vec<String>,
    rating: Option<Rating>,
    alternative_titles: Vec<AlternativeTitle>,
    translations: Vec<Translation>,
    images: Vec<Image>,
    seasons: Vec<Season>,
    episodes: Vec<Episode>,
}

/// `MapSeriesStatus`: "ended" and "upcoming", everything else continuing.
fn status(raw: Option<&str>) -> SeriesStatus {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("ended") => SeriesStatus::Ended,
        Some("upcoming") => SeriesStatus::Upcoming,
        _ => SeriesStatus::Continuing,
    }
}

/// TheTVDB has no series type; anime are recognised by the genre TheTVDB gives them.
fn is_anime(show: &Show) -> bool {
    show.genres.iter().any(|g| g.eq_ignore_ascii_case("anime"))
}

fn year(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

fn external_ids(show: &Show) -> Vec<ExternalId> {
    let mut ids = vec![ExternalId {
        kind: ExternalIdKind::Tvdb,
        value: show.tvdb_id.to_string(),
    }];
    let mut push = |kind, value: Option<i64>| {
        if let Some(v) = value.filter(|v| *v > 0) {
            ids.push(ExternalId {
                kind,
                value: v.to_string(),
            });
        }
    };
    push(ExternalIdKind::Tmdb, show.tmdb_id);
    push(ExternalIdKind::TvMaze, show.tv_maze_id);
    push(ExternalIdKind::TvRage, show.tv_rage_id);
    for id in &show.mal_ids {
        push(ExternalIdKind::MyAnimeList, Some(*id));
    }
    for id in &show.ani_list_ids {
        push(ExternalIdKind::AniList, Some(*id));
    }
    if let Some(imdb) = show.imdb_id.clone().filter(|v| v.starts_with("tt")) {
        ids.push(ExternalId {
            kind: ExternalIdKind::Imdb,
            value: imdb,
        });
    }
    ids
}

fn images(show: &Show) -> Vec<MediaImage> {
    show.images
        .iter()
        .filter(|i| !i.url.is_empty())
        .map(|i| MediaImage {
            cover_type: i.cover_type.to_ascii_lowercase(),
            remote_url: i.url.clone(),
        })
        .collect()
}

fn poster(show: &Show) -> Option<String> {
    show.images
        .iter()
        .find(|i| i.cover_type.eq_ignore_ascii_case("poster"))
        .map(|i| i.url.clone())
}

/// `MapSeries` + `MapEpisode`.
fn map_show(show: Show) -> SeriesInfo {
    let anime = is_anime(&show);
    let original_title = show
        .translations
        .iter()
        .find(|t| Some(&t.language) == show.original_language.as_ref())
        .and_then(|t| t.title.clone())
        .filter(|t| !t.trim().is_empty());
    let original_language = show
        .original_language
        .as_deref()
        .and_then(|l| kitsunarr_parser::find_iso(&l.to_lowercase()))
        .map(|l| l.language().id)
        .or(Some(1));
    let mut seasons: Vec<SeasonInfo> = Vec::new();
    for s in &show.seasons {
        if !seasons.iter().any(|x| x.season_number == s.season_number) {
            seasons.push(SeasonInfo {
                season_number: s.season_number,
                title: None,
            });
        }
    }
    let episodes = show
        .episodes
        .iter()
        .map(|e| EpisodeInfo {
            season_number: e.season_number,
            episode_number: e.episode_number,
            absolute_episode_number: e.absolute_episode_number,
            title: e.title.clone().filter(|t| !t.is_empty()),
            overview: e.overview.clone().filter(|t| !t.is_empty()),
            air_date: e.air_date.clone().filter(|t| !t.is_empty()),
            air_date_utc: e.air_date_utc.clone().filter(|t| !t.is_empty()),
            runtime: e.runtime,
            provider_episode_id: (e.tvdb_id > 0).then(|| e.tvdb_id.to_string()),
        })
        .collect();
    let mut alternate_titles: Vec<String> = Vec::new();
    for t in show
        .alternative_titles
        .iter()
        .map(|a| a.title.clone())
        .chain(original_title.clone())
    {
        if !t.trim().is_empty() && t != show.title && !alternate_titles.contains(&t) {
            alternate_titles.push(t);
        }
    }
    SeriesInfo {
        provider: P,
        id: show.tvdb_id.to_string(),
        kind: if anime {
            EntryKind::Anime
        } else {
            EntryKind::Series
        },
        title: show.title.clone(),
        alternate_titles,
        original_title,
        year: year(show.first_aired.as_deref()),
        status: status(show.status.as_deref()),
        series_type: if anime {
            SeriesType::Anime
        } else {
            SeriesType::Standard
        },
        overview: show.overview.clone().filter(|o| !o.is_empty()),
        network: show.network.clone().filter(|n| !n.is_empty()),
        air_time: show
            .time_of_day
            .as_ref()
            .and_then(|t| Some(format!("{:02}:{:02}", t.hours?, t.minutes.unwrap_or(0)))),
        runtime: show.runtime,
        genres: show.genres.clone(),
        images: images(&show),
        rating: show
            .rating
            .as_ref()
            .and_then(|r| r.value.as_deref())
            .and_then(|v| v.parse().ok()),
        votes: show.rating.as_ref().and_then(|r| r.count),
        first_aired: show.first_aired.clone(),
        last_aired: show.last_aired.clone(),
        original_language,
        external_ids: external_ids(&show),
        seasons,
        episodes,
    }
}

impl SkyHook {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base(http, DEFAULT_BASE)
    }

    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            // SkyHook publishes no limit; stay polite.
            limiter: RateLimiter::new(10, Duration::from_secs(10)),
            cache: TtlCache::new(Duration::from_secs(600), 64),
            language: std::sync::Arc::new(std::sync::RwLock::new(DEFAULT_LANGUAGE.into())),
        }
    }

    /// Sets the preferred language (an ISO 639-2 code; English when empty).
    pub fn set_language(&self, code: &str) {
        let code = code.trim().to_ascii_lowercase();
        *self.language.write().unwrap_or_else(|e| e.into_inner()) = if code.len() == 3 {
            code
        } else {
            DEFAULT_LANGUAGE.into()
        };
    }

    fn language_or_preferred(&self, language: Option<&str>) -> String {
        language
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| l.len() == 3)
            .unwrap_or_else(|| {
                self.language
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            })
    }

    /// `SearchForNewSeries`: `tvdb:{id}` (or `tvdbid:`) looks the title up directly; other
    /// terms go to SkyHook's search, lower-cased as upstream sends them.
    pub async fn search(&self, term: &str) -> MetadataResult<Vec<SearchResult>> {
        let trimmed = term.trim();
        let lower = trimmed.to_lowercase();
        if let Some(id) = lower
            .strip_prefix("tvdb:")
            .or_else(|| lower.strip_prefix("tvdbid:"))
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        {
            return match self.series(id).await {
                Ok(info) => Ok(vec![crate::result_of(&info)]),
                Err(crate::MetadataError::NotFound(_)) => Ok(Vec::new()),
                Err(e) => Err(e),
            };
        }
        let url = format!("{}/search/", self.base);
        let language = self.language_or_preferred(None);
        let shows: Vec<Show> = send_json(P, &self.limiter, || {
            self.http
                .get(&url)
                .query(&[("language", language.as_str()), ("term", lower.as_str())])
        })
        .await?;
        Ok(shows
            .into_iter()
            .filter(|s| s.tvdb_id > 0)
            .map(|s| SearchResult {
                provider: P,
                id: s.tvdb_id.to_string(),
                kind: if is_anime(&s) {
                    EntryKind::Anime
                } else {
                    EntryKind::Series
                },
                title: s.title.clone(),
                year: year(s.first_aired.as_deref()),
                overview: s.overview.clone().filter(|o| !o.is_empty()),
                network: s.network.clone().filter(|n| !n.is_empty()),
                status: status(s.status.as_deref()),
                poster_url: poster(&s),
                external_ids: external_ids(&s),
            })
            .collect())
    }

    pub fn forget(&self, id: &str) {
        for ordering in [
            "official",
            "dvd",
            "absolute",
            "alternate",
            "regional",
            "altdvd",
        ] {
            self.cache.remove(&format!("{id}/{ordering}"));
        }
    }

    /// The series in TheTVDB's aired order.
    pub async fn series(&self, id: &str) -> MetadataResult<SeriesInfo> {
        self.series_ordered(id, "official").await
    }

    /// `GetSeriesInfo(tvdbId, language, seasonType)` in the preferred language.
    pub async fn series_ordered(&self, id: &str, season_type: &str) -> MetadataResult<SeriesInfo> {
        self.series_in(id, season_type, None).await
    }

    /// `GetSeriesInfo(tvdbId, language, seasonType)`: `language` is an ISO 639-2 code (the
    /// preferred language when `None`).
    pub async fn series_in(
        &self,
        id: &str,
        season_type: &str,
        language: Option<&str>,
    ) -> MetadataResult<SeriesInfo> {
        let season_type = if season_type.trim().is_empty() {
            "official"
        } else {
            season_type.trim()
        };
        let language = self.language_or_preferred(language);
        let key = format!("{id}/{season_type}");
        if let Some((cached_language, hit)) = self.cache.get(&key)
            && cached_language == language
        {
            return Ok(hit);
        }
        let tvdb_id: i64 = id
            .trim()
            .parse()
            .ok()
            .filter(|v| *v > 0)
            .ok_or(crate::MetadataError::NotFound(P))?;
        let url = format!("{}/shows/{language}/{tvdb_id}/{season_type}", self.base);
        let show: Show = send_json(P, &self.limiter, || self.http.get(&url)).await?;
        if show.tvdb_id != tvdb_id {
            return Err(crate::MetadataError::Decode {
                provider: P,
                message: format!("asked for {tvdb_id}, got {}", show.tvdb_id),
            });
        }
        let info = map_show(show);
        self.cache.insert(key, (language, info.clone()));
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/../../tests/fixtures/metadata/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn maps_a_real_show() {
        let show: Show = serde_json::from_str(&fixture("skyhook-show-81189.json")).unwrap();
        let info = map_show(show);
        assert_eq!(
            (info.title.as_str(), info.year, info.status),
            ("Breaking Bad", Some(2008), SeriesStatus::Ended)
        );
        assert_eq!(info.kind, EntryKind::Series);
        assert_eq!(info.network.as_deref(), Some("AMC"));
        assert_eq!(info.air_time.as_deref(), Some("21:00"));
        assert_eq!(info.runtime, Some(48));
        assert_eq!(info.original_language, Some(1));
        assert!(info.episodes.len() > 60);
        let pilot = info
            .episodes
            .iter()
            .find(|e| e.season_number == 1 && e.episode_number == 1)
            .unwrap();
        assert_eq!(pilot.title.as_deref(), Some("Pilot"));
        assert_eq!(pilot.air_date.as_deref(), Some("2008-01-20"));
        assert_eq!(pilot.provider_episode_id.as_deref(), Some("349232"));
        let special = info.episodes.iter().find(|e| e.season_number == 0).unwrap();
        assert_eq!(special.title.as_deref(), Some("Good Cop / Bad Cop"));
        assert!(info.seasons.iter().any(|s| s.season_number == 0));
        let ids: Vec<(ExternalIdKind, &str)> = info
            .external_ids
            .iter()
            .map(|i| (i.kind, i.value.as_str()))
            .collect();
        assert!(ids.contains(&(ExternalIdKind::Tvdb, "81189")));
        assert!(ids.contains(&(ExternalIdKind::Tmdb, "1396")));
        assert!(ids.contains(&(ExternalIdKind::TvMaze, "169")));
        assert!(ids.contains(&(ExternalIdKind::Imdb, "tt0903747")));
        assert!(info.images.iter().any(|i| i.cover_type == "poster"));
        assert_eq!(info.rating, Some(9.5));
        assert!(info.votes.is_some_and(|v| v > 1000));
    }

    #[test]
    fn maps_anime_with_absolute_numbers_and_cross_ids() {
        let show: Show = serde_json::from_str(&fixture("skyhook-show-76885.json")).unwrap();
        let info = map_show(show);
        assert_eq!(info.kind, EntryKind::Anime);
        assert_eq!(info.series_type, SeriesType::Anime);
        assert_eq!(info.air_time.as_deref(), Some("17:00"));
        assert_eq!(
            info.original_language,
            kitsunarr_domain::Language::by_name("Japanese").map(|l| l.id)
        );
        assert!(
            info.episodes
                .iter()
                .filter(|e| e.season_number == 1)
                .all(|e| e.absolute_episode_number.is_some())
        );
        assert!(
            info.external_ids
                .iter()
                .any(|i| i.kind == ExternalIdKind::MyAnimeList && i.value == "1")
        );
        assert!(
            info.alternate_titles
                .contains(&"Cowboy Bebop (1998)".to_string())
        );
    }

    #[test]
    fn search_results_keep_their_ids() {
        let shows: Vec<Show> =
            serde_json::from_str(&fixture("skyhook-search-breaking-bad.json")).unwrap();
        assert_eq!(shows[0].title, "Breaking Bad");
        assert_eq!(shows[0].tvdb_id, 81189);
        assert!(poster(&shows[0]).is_some());
    }

    #[test]
    fn statuses_follow_upstream() {
        assert_eq!(status(Some("Ended")), SeriesStatus::Ended);
        assert_eq!(status(Some("upcoming")), SeriesStatus::Upcoming);
        assert_eq!(status(Some("Continuing")), SeriesStatus::Continuing);
        assert_eq!(status(None), SeriesStatus::Continuing);
    }
}
