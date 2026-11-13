//! TVmaze (<https://www.tvmaze.com/api>): TV series, no API key, 20 requests / 10 s per IP.
//! Data is licensed CC BY-SA 4.0; Kitsunarr credits TVmaze in the UI.

use std::time::Duration;

use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MetadataProvider, SeriesStatus, SeriesType,
};
use serde::Deserialize;

use crate::{
    EntryKind, EpisodeInfo, MetadataResult, RateLimiter, SearchResult, SeasonInfo, SeriesInfo,
    TtlCache, send_json, strip_html,
};

const P: MetadataProvider = MetadataProvider::TvMaze;
pub const DEFAULT_BASE: &str = "https://api.tvmaze.com";

#[derive(Clone)]
pub struct TvMaze {
    http: reqwest::Client,
    base: String,
    limiter: RateLimiter,
    cache: TtlCache<SeriesInfo>,
}

#[derive(Deserialize)]
struct SearchHit {
    show: Show,
}

#[derive(Deserialize, Clone)]
struct Named {
    name: String,
}

#[derive(Deserialize, Clone, Default)]
struct Externals {
    tvrage: Option<i64>,
    thetvdb: Option<i64>,
    imdb: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
struct Image {
    medium: Option<String>,
    original: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
struct Rating {
    average: Option<f64>,
}

#[derive(Deserialize, Clone, Default)]
struct Schedule {
    time: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Show {
    id: i64,
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    language: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    status: Option<String>,
    runtime: Option<i32>,
    #[serde(rename = "averageRuntime")]
    average_runtime: Option<i32>,
    premiered: Option<String>,
    ended: Option<String>,
    #[serde(default)]
    schedule: Schedule,
    #[serde(default)]
    rating: Rating,
    network: Option<Named>,
    #[serde(rename = "webChannel")]
    web_channel: Option<Named>,
    #[serde(default)]
    externals: Externals,
    image: Option<Image>,
    summary: Option<String>,
    #[serde(rename = "_embedded")]
    embedded: Option<Embedded>,
}

#[derive(Deserialize, Clone, Default)]
struct Embedded {
    #[serde(default)]
    seasons: Vec<Season>,
}

#[derive(Deserialize, Clone)]
struct Episode {
    id: i64,
    name: Option<String>,
    season: i32,
    number: Option<i32>,
    airdate: Option<String>,
    airstamp: Option<String>,
    runtime: Option<i32>,
    summary: Option<String>,
    #[serde(default)]
    image: Option<Image>,
}

#[derive(Deserialize, Clone)]
struct Season {
    number: i32,
    name: Option<String>,
}

fn status(raw: Option<&str>) -> SeriesStatus {
    match raw {
        Some("Running") | Some("To Be Determined") => SeriesStatus::Continuing,
        Some("Ended") => SeriesStatus::Ended,
        Some("In Development") => SeriesStatus::Upcoming,
        _ => SeriesStatus::Unknown,
    }
}

fn is_anime(show: &Show) -> bool {
    show.kind.as_deref() == Some("Animation") && show.language.as_deref() == Some("Japanese")
}

fn year(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

fn external_ids(show: &Show) -> Vec<ExternalId> {
    let mut ids = vec![ExternalId {
        kind: ExternalIdKind::TvMaze,
        value: show.id.to_string(),
    }];
    if let Some(v) = show.externals.thetvdb.filter(|v| *v > 0) {
        ids.push(ExternalId {
            kind: ExternalIdKind::Tvdb,
            value: v.to_string(),
        });
    }
    if let Some(v) = show.externals.imdb.clone().filter(|v| !v.is_empty()) {
        ids.push(ExternalId {
            kind: ExternalIdKind::Imdb,
            value: v,
        });
    }
    if let Some(v) = show.externals.tvrage.filter(|v| *v > 0) {
        ids.push(ExternalId {
            kind: ExternalIdKind::TvRage,
            value: v.to_string(),
        });
    }
    ids
}

fn to_utc(stamp: Option<&str>) -> Option<String> {
    let t = chrono::DateTime::parse_from_rfc3339(stamp?).ok()?;
    Some(
        t.with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// Upper bound on a show's episode list (TVmaze's longest-running shows have a few thousand).
const MAX_EPISODES: usize = 20_000;

/// Maps a show and its episode list (fetched with `specials=1`).
///
/// Numbering follows Tanukarr so refreshed and migrated rows line up: TVmaze season/number
/// as is, unnumbered specials in season 0 numbered 1..n by TVmaze episode id, and no
/// absolute numbers (TVmaze has none). An inconsistent list is rejected so a refresh keeps
/// the existing episodes instead of overwriting them with bad data.
fn map_show(show: Show, all_episodes: Vec<Episode>) -> MetadataResult<SeriesInfo> {
    let invalid = |message: String| crate::MetadataError::Decode {
        provider: P,
        message,
    };
    if all_episodes.len() > MAX_EPISODES {
        return Err(invalid(format!(
            "{} episodes exceed the limit of {MAX_EPISODES}",
            all_episodes.len()
        )));
    }
    let mut ids = std::collections::HashSet::new();
    if let Some(dup) = all_episodes.iter().find(|e| !ids.insert(e.id)) {
        return Err(invalid(format!("episode id {} is listed twice", dup.id)));
    }
    if let Some(neg) = all_episodes.iter().find(|e| e.season < 0) {
        return Err(invalid(format!("episode {} has a negative season", neg.id)));
    }
    let mut numbered: Vec<&Episode> = all_episodes.iter().filter(|e| e.number.is_some()).collect();
    numbered.sort_by_key(|e| (e.season, e.number, e.id));
    let mut specials: Vec<&Episode> = all_episodes.iter().filter(|e| e.number.is_none()).collect();
    specials.sort_by_key(|e| e.id);
    let numbered_specials = specials
        .iter()
        .enumerate()
        .map(|(i, e)| (0, i as i32 + 1, *e));
    let mut episodes = Vec::with_capacity(all_episodes.len());
    let mut slots = std::collections::HashSet::new();
    for (season, number, e) in numbered
        .iter()
        .map(|e| (e.season, e.number.unwrap_or_default(), *e))
        .chain(numbered_specials)
    {
        if !slots.insert((season, number)) {
            return Err(invalid(format!("S{season:02}E{number:02} is listed twice")));
        }
        episodes.push(EpisodeInfo {
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            title: e.name.clone(),
            overview: e
                .summary
                .as_deref()
                .map(strip_html)
                .filter(|s| !s.is_empty()),
            air_date: e.airdate.clone().filter(|d| !d.is_empty()),
            air_date_utc: to_utc(e.airstamp.as_deref()),
            runtime: e.runtime,
            provider_episode_id: Some(e.id.to_string()),
            image: e.image.as_ref().and_then(|i| i.original.clone()),
        });
    }
    let embedded = show.embedded.clone().unwrap_or_default();
    let mut seasons: Vec<SeasonInfo> = embedded
        .seasons
        .iter()
        .map(|s| SeasonInfo {
            season_number: s.number,
            title: s.name.clone().filter(|n| !n.is_empty()),
            images: Vec::new(),
        })
        .collect();
    if !specials.is_empty() && !seasons.iter().any(|s| s.season_number == 0) {
        seasons.insert(
            0,
            SeasonInfo {
                season_number: 0,
                title: Some("Specials".into()),
                images: Vec::new(),
            },
        );
    }
    let mut images = Vec::new();
    if let Some(img) = show
        .image
        .as_ref()
        .and_then(|i| i.original.clone().or_else(|| i.medium.clone()))
    {
        images.push(MediaImage {
            cover_type: "poster".into(),
            remote_url: img,
        });
    }
    let anime = is_anime(&show);
    Ok(SeriesInfo {
        provider: P,
        id: show.id.to_string(),
        kind: if anime {
            EntryKind::Anime
        } else {
            EntryKind::Series
        },
        title: show.name.clone(),
        alternate_titles: Vec::new(),
        original_title: None,
        year: year(show.premiered.as_deref()),
        status: status(show.status.as_deref()),
        series_type: if anime {
            SeriesType::Anime
        } else {
            SeriesType::Standard
        },
        overview: show
            .summary
            .as_deref()
            .map(strip_html)
            .filter(|s| !s.is_empty()),
        network: show
            .network
            .as_ref()
            .or(show.web_channel.as_ref())
            .map(|n| n.name.clone()),
        air_time: show.schedule.time.clone().filter(|t| !t.is_empty()),
        runtime: show.runtime.or(show.average_runtime),
        genres: show.genres.clone(),
        images,
        rating: show.rating.average,
        votes: None,
        first_aired: show.premiered.clone(),
        last_aired: show.ended.clone(),
        original_language: show
            .language
            .as_deref()
            .and_then(|l| kitsunarr_domain::Language::by_name(l).map(|l| l.id)),
        original_country: None,
        external_ids: external_ids(&show),
        seasons,
        episodes,
    })
}

impl TvMaze {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base(http, DEFAULT_BASE)
    }

    /// Alternative base URL (tests).
    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            limiter: RateLimiter::new(18, Duration::from_secs(10)),
            cache: TtlCache::new(Duration::from_secs(600), 64),
        }
    }

    pub async fn search(&self, term: &str) -> MetadataResult<Vec<SearchResult>> {
        let url = format!("{}/search/shows", self.base);
        let hits: Vec<SearchHit> = send_json(P, &self.limiter, || {
            self.http.get(&url).query(&[("q", term)])
        })
        .await?;
        Ok(hits
            .into_iter()
            .map(|h| {
                let s = h.show;
                SearchResult {
                    provider: P,
                    id: s.id.to_string(),
                    kind: if is_anime(&s) {
                        EntryKind::Anime
                    } else {
                        EntryKind::Series
                    },
                    title: s.name.clone(),
                    year: year(s.premiered.as_deref()),
                    overview: s
                        .summary
                        .as_deref()
                        .map(strip_html)
                        .filter(|v| !v.is_empty()),
                    network: s
                        .network
                        .as_ref()
                        .or(s.web_channel.as_ref())
                        .map(|n| n.name.clone()),
                    status: status(s.status.as_deref()),
                    poster_url: s
                        .image
                        .as_ref()
                        .and_then(|i| i.medium.clone().or_else(|| i.original.clone())),
                    external_ids: external_ids(&s),
                }
            })
            .collect())
    }

    /// Drops a cached entry so the next [`Self::series`] call fetches fresh data.
    pub fn forget(&self, id: &str) {
        self.cache.remove(id);
    }

    pub async fn series(&self, id: &str) -> MetadataResult<SeriesInfo> {
        if let Some(hit) = self.cache.get(id) {
            return Ok(hit);
        }
        let id_num: i64 = id.parse().map_err(|_| crate::MetadataError::NotFound(P))?;
        let show_url = format!("{}/shows/{id_num}", self.base);
        let show: Show = send_json(P, &self.limiter, || {
            self.http.get(&show_url).query(&[("embed[]", "seasons")])
        })
        .await?;
        let episodes_url = format!("{}/shows/{id_num}/episodes", self.base);
        let episodes: Vec<Episode> = send_json(P, &self.limiter, || {
            self.http.get(&episodes_url).query(&[("specials", "1")])
        })
        .await?;
        let info = map_show(show, episodes)?;
        self.cache.insert(id.to_string(), info.clone());
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

    fn real_show() -> (Show, Vec<Episode>) {
        let raw: serde_json::Value =
            serde_json::from_str(&fixture("tvmaze-show-169.json")).unwrap();
        let episodes = serde_json::from_value(raw["_embedded"]["episodes"].clone()).unwrap();
        (serde_json::from_value(raw).unwrap(), episodes)
    }

    #[test]
    fn maps_a_real_show_response() {
        let (show, episodes) = real_show();
        let info = map_show(show, episodes).unwrap();
        assert_eq!(
            (info.title.as_str(), info.year, info.status),
            ("Breaking Bad", Some(2008), SeriesStatus::Ended)
        );
        assert_eq!(info.network.as_deref(), Some("AMC"));
        assert_eq!(info.episodes.len(), 62);
        assert_eq!(info.seasons.len(), 5);
        let pilot = &info.episodes[0];
        assert_eq!(
            (
                pilot.season_number,
                pilot.episode_number,
                pilot.absolute_episode_number
            ),
            (1, 1, None),
            "TVmaze has no absolute numbers"
        );
        assert_eq!(
            pilot.air_date_utc.as_deref(),
            Some("2008-01-21T03:00:00Z"),
            "airstamp converted to UTC"
        );
        assert!(info.external_ids.contains(&ExternalId {
            kind: ExternalIdKind::Tvdb,
            value: "81189".into()
        }));
        assert!(info.external_ids.contains(&ExternalId {
            kind: ExternalIdKind::Imdb,
            value: "tt0903747".into()
        }));
        assert_eq!(info.series_type, SeriesType::Standard);
    }

    fn special(id: i64, season: i32, name: &str) -> Episode {
        Episode {
            id,
            name: Some(name.into()),
            season,
            number: None,
            airdate: None,
            airstamp: None,
            runtime: None,
            summary: None,
            image: None,
        }
    }

    #[test]
    fn specials_go_to_season_zero_in_episode_id_order() {
        let (show, mut episodes) = real_show();
        // Aired in a different order than their ids: numbering must follow the id (Tanukarr parity).
        episodes.push(special(900, 5, "Later id"));
        episodes.push(special(100, 2, "Earlier id"));
        let info = map_show(show, episodes).unwrap();
        let s0: Vec<_> = info
            .episodes
            .iter()
            .filter(|e| e.season_number == 0)
            .map(|e| (e.episode_number, e.title.clone().unwrap()))
            .collect();
        assert_eq!(
            s0,
            vec![(1, "Earlier id".to_string()), (2, "Later id".to_string())]
        );
        assert_eq!(info.seasons[0].season_number, 0);
        assert_eq!(info.episodes.len(), 64);
    }

    #[test]
    fn inconsistent_episode_lists_are_rejected() {
        let (show, episodes) = real_show();
        let mut dup_slot = episodes.clone();
        let mut copy = dup_slot[0].clone();
        copy.id = 1;
        dup_slot.push(copy);
        assert!(matches!(
            map_show(show.clone(), dup_slot),
            Err(crate::MetadataError::Decode { .. })
        ));
        let mut dup_id = episodes.clone();
        dup_id.push(episodes[3].clone());
        assert!(matches!(
            map_show(show.clone(), dup_id),
            Err(crate::MetadataError::Decode { .. })
        ));
        let mut negative = episodes;
        negative[0].season = -1;
        assert!(matches!(
            map_show(show, negative),
            Err(crate::MetadataError::Decode { .. })
        ));
    }

    #[test]
    fn maps_search_results() {
        let hits: Vec<SearchHit> = serde_json::from_str(&fixture("tvmaze-search.json")).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].show.name, "Breaking Bad");
    }
}
