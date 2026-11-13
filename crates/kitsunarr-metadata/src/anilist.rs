//! AniList (<https://docs.anilist.co>): anime series and films through GraphQL, no API key.
//! The public limit is 90 requests per minute (30 while AniList runs in degraded mode), so
//! Kitsunarr stays below 25.

use std::time::Duration;

use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MetadataProvider, SeriesStatus, SeriesType,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    EntryKind, EpisodeInfo, MetadataError, MetadataResult, RateLimiter, SearchResult, SeasonInfo,
    SeriesInfo, TtlCache, send_json, strip_html,
};

const P: MetadataProvider = MetadataProvider::AniList;
pub const DEFAULT_BASE: &str = "https://graphql.anilist.co";
const MAX_SCHEDULE_PAGES: u32 = 20;

const FIELDS: &str = "id idMal format status episodes duration seasonYear countryOfOrigin \
    title { romaji english native } synonyms startDate { year month day } endDate { year month day } \
    coverImage { extraLarge large } bannerImage genres averageScore popularity description(asHtml: false) \
    studios(isMain: true) { nodes { name } } nextAiringEpisode { episode airingAt }";

#[derive(Clone)]
pub struct AniList {
    http: reqwest::Client,
    base: String,
    limiter: RateLimiter,
    cache: TtlCache<SeriesInfo>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
    status: Option<u16>,
}

#[derive(Deserialize)]
struct PageData {
    #[serde(rename = "Page")]
    page: MediaPage,
}

#[derive(Deserialize)]
struct MediaPage {
    #[serde(default)]
    media: Vec<Media>,
}

#[derive(Deserialize)]
struct MediaData {
    #[serde(rename = "Media")]
    media: Option<Media>,
}

#[derive(Deserialize, Clone, Default)]
struct Title {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
struct FuzzyDate {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
}

impl FuzzyDate {
    fn iso(&self) -> Option<String> {
        Some(format!(
            "{:04}-{:02}-{:02}",
            self.year?,
            self.month.unwrap_or(1),
            self.day.unwrap_or(1)
        ))
    }
}

#[derive(Deserialize, Clone, Default)]
struct Cover {
    #[serde(rename = "extraLarge")]
    extra_large: Option<String>,
    large: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
struct Studios {
    #[serde(default)]
    nodes: Vec<Studio>,
}

#[derive(Deserialize, Clone)]
struct Studio {
    name: String,
}

#[derive(Deserialize, Clone)]
struct Airing {
    episode: i32,
    #[serde(rename = "airingAt")]
    airing_at: i64,
}

#[derive(Deserialize, Clone)]
struct Media {
    id: i64,
    #[serde(rename = "idMal")]
    id_mal: Option<i64>,
    format: Option<String>,
    status: Option<String>,
    episodes: Option<i32>,
    duration: Option<i32>,
    #[serde(rename = "seasonYear")]
    season_year: Option<i32>,
    #[serde(rename = "countryOfOrigin")]
    country: Option<String>,
    #[serde(default)]
    title: Title,
    #[serde(default)]
    synonyms: Vec<String>,
    #[serde(rename = "startDate", default)]
    start: FuzzyDate,
    #[serde(rename = "endDate", default)]
    end: FuzzyDate,
    #[serde(rename = "coverImage", default)]
    cover: Cover,
    #[serde(rename = "bannerImage")]
    banner: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(rename = "averageScore")]
    average_score: Option<f64>,
    popularity: Option<i64>,
    description: Option<String>,
    #[serde(default)]
    studios: Studios,
    #[serde(rename = "nextAiringEpisode")]
    next_airing: Option<Airing>,
}

#[derive(Deserialize)]
struct ScheduleData {
    #[serde(rename = "Media")]
    media: Option<ScheduleMedia>,
}

#[derive(Deserialize)]
struct ScheduleMedia {
    #[serde(rename = "airingSchedule")]
    schedule: SchedulePage,
}

#[derive(Deserialize)]
struct SchedulePage {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    #[serde(default)]
    nodes: Vec<Airing>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next: Option<bool>,
}

fn status(raw: Option<&str>) -> SeriesStatus {
    match raw {
        Some("FINISHED") | Some("CANCELLED") => SeriesStatus::Ended,
        Some("RELEASING") | Some("HIATUS") => SeriesStatus::Continuing,
        Some("NOT_YET_RELEASED") => SeriesStatus::Upcoming,
        _ => SeriesStatus::Unknown,
    }
}

fn kind(m: &Media) -> EntryKind {
    if m.format.as_deref() == Some("MOVIE") {
        EntryKind::Movie
    } else {
        EntryKind::Anime
    }
}

fn titles(m: &Media) -> (String, Vec<String>, Option<String>) {
    let primary = m
        .title
        .english
        .clone()
        .or_else(|| m.title.romaji.clone())
        .or_else(|| m.title.native.clone())
        .unwrap_or_else(|| format!("AniList {}", m.id));
    let mut alts: Vec<String> = Vec::new();
    for t in [
        m.title.romaji.clone(),
        m.title.english.clone(),
        m.title.native.clone(),
    ]
    .into_iter()
    .flatten()
    .chain(m.synonyms.iter().cloned())
    {
        if t != primary && !alts.contains(&t) {
            alts.push(t);
        }
    }
    (primary, alts, m.title.native.clone())
}

fn language(country: Option<&str>) -> Option<i32> {
    match country? {
        "JP" => Some(8),
        "CN" | "TW" => Some(10),
        "KR" => Some(21),
        _ => None,
    }
}

fn external_ids(m: &Media) -> Vec<ExternalId> {
    let mut ids = vec![ExternalId {
        kind: ExternalIdKind::AniList,
        value: m.id.to_string(),
    }];
    if let Some(mal) = m.id_mal.filter(|v| *v > 0) {
        ids.push(ExternalId {
            kind: ExternalIdKind::MyAnimeList,
            value: mal.to_string(),
        });
    }
    ids
}

fn map_media(m: Media, schedule: Vec<Airing>) -> SeriesInfo {
    let (title, alternate_titles, original_title) = titles(&m);
    // Known episodes: declared count, else what has aired or is scheduled.
    let scheduled_max = schedule.iter().map(|a| a.episode).max().unwrap_or(0);
    let aired_max = m.next_airing.as_ref().map_or(0, |n| n.episode - 1);
    let count = m
        .episodes
        .unwrap_or(0)
        .max(scheduled_max)
        .max(aired_max)
        .max(i32::from(kind(&m) == EntryKind::Movie));
    let start_date = m.start.iso();
    let episodes = (1..=count)
        .map(|n| {
            let air = schedule
                .iter()
                .find(|a| a.episode == n)
                .and_then(|a| chrono::DateTime::from_timestamp(a.airing_at, 0));
            let movie_date = (count == 1).then(|| start_date.clone()).flatten();
            EpisodeInfo {
                season_number: 1,
                episode_number: n,
                absolute_episode_number: Some(n),
                title: None,
                overview: None,
                air_date: air
                    .map(|t| t.format("%Y-%m-%d").to_string())
                    .or(movie_date.clone()),
                air_date_utc: air
                    .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                    .or(movie_date.map(|d| format!("{d}T00:00:00Z"))),
                runtime: m.duration,
                provider_episode_id: Some(format!("{}:{n}", m.id)),
            }
        })
        .collect();
    let mut images = Vec::new();
    if let Some(poster) = m
        .cover
        .extra_large
        .clone()
        .or_else(|| m.cover.large.clone())
    {
        images.push(MediaImage {
            cover_type: "poster".into(),
            remote_url: poster,
        });
    }
    if let Some(banner) = m.banner.clone() {
        images.push(MediaImage {
            cover_type: "banner".into(),
            remote_url: banner,
        });
    }
    SeriesInfo {
        provider: P,
        id: m.id.to_string(),
        kind: kind(&m),
        title,
        alternate_titles,
        original_title,
        year: m.season_year.or(m.start.year),
        status: status(m.status.as_deref()),
        series_type: SeriesType::Anime,
        overview: m
            .description
            .as_deref()
            .map(strip_html)
            .filter(|s| !s.is_empty()),
        network: m.studios.nodes.first().map(|s| s.name.clone()),
        air_time: None,
        runtime: m.duration,
        genres: m.genres.clone(),
        images,
        rating: m.average_score.map(|s| s / 10.0),
        votes: m.popularity,
        first_aired: start_date,
        last_aired: m.end.iso(),
        original_language: language(m.country.as_deref()),
        original_country: m.country.clone(),
        external_ids: external_ids(&m),
        seasons: vec![SeasonInfo {
            season_number: 1,
            title: None,
        }],
        episodes,
    }
}

impl AniList {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base(http, DEFAULT_BASE)
    }

    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            limiter: RateLimiter::new(25, Duration::from_secs(60)),
            cache: TtlCache::new(Duration::from_secs(600), 64),
        }
    }

    async fn query<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> MetadataResult<T> {
        let body = json!({ "query": query, "variables": variables });
        let env: Envelope<T> = send_json(P, &self.limiter, || {
            self.http
                .post(&self.base)
                .header("Accept", "application/json")
                .json(&body)
        })
        .await?;
        if let Some(e) = env.errors.first() {
            if e.status == Some(404) {
                return Err(MetadataError::NotFound(P));
            }
            return Err(MetadataError::Http {
                provider: P,
                message: e.message.clone(),
            });
        }
        env.data.ok_or_else(|| MetadataError::Decode {
            provider: P,
            message: "empty response".into(),
        })
    }

    pub async fn search(&self, term: &str) -> MetadataResult<Vec<SearchResult>> {
        let q = format!(
            "query ($search: String) {{ Page(perPage: 20) {{ media(search: $search, type: ANIME, sort: SEARCH_MATCH, isAdult: false) {{ {FIELDS} }} }} }}"
        );
        let data: PageData = self.query(&q, json!({ "search": term })).await?;
        Ok(data
            .page
            .media
            .into_iter()
            .map(|m| {
                let (title, _, _) = titles(&m);
                SearchResult {
                    provider: P,
                    id: m.id.to_string(),
                    kind: kind(&m),
                    year: m.season_year.or(m.start.year),
                    overview: m
                        .description
                        .as_deref()
                        .map(strip_html)
                        .filter(|s| !s.is_empty()),
                    network: m.studios.nodes.first().map(|s| s.name.clone()),
                    status: status(m.status.as_deref()),
                    poster_url: m
                        .cover
                        .large
                        .clone()
                        .or_else(|| m.cover.extra_large.clone()),
                    external_ids: external_ids(&m),
                    title,
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
        let id_num: i64 = id.parse().map_err(|_| MetadataError::NotFound(P))?;
        let q = format!("query ($id: Int) {{ Media(id: $id, type: ANIME) {{ {FIELDS} }} }}");
        let data: MediaData = self.query(&q, json!({ "id": id_num })).await?;
        let media = data.media.ok_or(MetadataError::NotFound(P))?;
        let mut schedule = Vec::new();
        for page in 1..=MAX_SCHEDULE_PAGES {
            let q = "query ($id: Int, $page: Int) { Media(id: $id, type: ANIME) { airingSchedule(page: $page, perPage: 50) { pageInfo { hasNextPage } nodes { episode airingAt } } } }";
            let data: ScheduleData = self.query(q, json!({ "id": id_num, "page": page })).await?;
            let Some(sm) = data.media else { break };
            let more = sm.schedule.page_info.has_next.unwrap_or(false);
            schedule.extend(sm.schedule.nodes);
            if !more {
                break;
            }
        }
        let info = map_media(media, schedule);
        self.cache.insert(id.to_string(), info.clone());
        Ok(info)
    }
}

/// An anime's format, title and its PREQUEL/SEQUEL/PARENT/SIDE_STORY relations (Tanukarr's
/// `AniListProxy.GetAnimeRelations`), for grouping a franchise.
pub async fn relations(client: &AniList, id: i64) -> MetadataResult<crate::AnimeNode> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Title {
        english: Option<String>,
        romaji: Option<String>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Node {
        id: i64,
        #[serde(rename = "type")]
        kind: Option<String>,
        format: Option<String>,
        title: Title,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Edge {
        relation_type: Option<String>,
        node: Node,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Edges {
        edges: Vec<Edge>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Media {
        id: i64,
        format: Option<String>,
        title: Title,
        relations: Edges,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Data {
        #[serde(rename = "Media")]
        media: Option<Media>,
    }
    let q = "query ($id: Int) { Media(id: $id, type: ANIME) { id format title { english romaji } relations { edges { relationType(version: 2) node { id type format title { english romaji } } } } } }";
    let data: Data = client.query(q, json!({ "id": id })).await?;
    let m = data.media.ok_or(MetadataError::NotFound(P))?;
    let title = |t: &Title| {
        t.english
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| t.romaji.clone())
            .unwrap_or_default()
    };
    Ok(crate::AnimeNode {
        id: m.id,
        format: m.format.unwrap_or_default(),
        title: title(&m.title),
        relations: m
            .relations
            .edges
            .into_iter()
            .filter(|e| e.node.kind.as_deref() == Some("ANIME") && e.node.id > 0)
            .filter_map(|e| {
                let kind = e.relation_type?;
                matches!(
                    kind.as_str(),
                    "PREQUEL" | "SEQUEL" | "PARENT" | "SIDE_STORY"
                )
                .then(|| crate::AnimeRelation {
                    kind,
                    id: e.node.id,
                    format: e.node.format.clone().unwrap_or_default(),
                    title: title(&e.node.title),
                })
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(format: &str, episodes: Option<i32>) -> Media {
        serde_json::from_value(json!({
            "id": 1, "idMal": 1, "format": format, "status": "FINISHED", "episodes": episodes, "duration": 24,
            "seasonYear": 1998, "countryOfOrigin": "JP",
            "title": {"romaji": "Cowboy Bebop", "english": "Cowboy Bebop", "native": "カウボーイビバップ"},
            "synonyms": ["CB"], "startDate": {"year": 1998, "month": 4, "day": 3}, "endDate": {"year": 1999, "month": 4, "day": 24},
            "coverImage": {"extraLarge": "https://s4.anilist.co/x.jpg"}, "genres": ["Action"], "averageScore": 86,
            "popularity": 400000, "description": "Space <i>bounty</i> hunters.<br>",
            "studios": {"nodes": [{"name": "Sunrise"}]}, "nextAiringEpisode": null
        }))
        .unwrap()
    }

    #[test]
    fn maps_series_and_schedule() {
        let info = map_media(
            media("TV", Some(26)),
            vec![Airing {
                episode: 1,
                airing_at: 891_590_400,
            }],
        );
        assert_eq!(info.title, "Cowboy Bebop");
        assert_eq!(
            info.alternate_titles,
            vec!["カウボーイビバップ".to_string(), "CB".into()]
        );
        assert_eq!(info.episodes.len(), 26);
        assert_eq!(info.episodes[0].air_date.as_deref(), Some("1998-04-03"));
        assert_eq!(info.episodes[0].provider_episode_id.as_deref(), Some("1:1"));
        assert_eq!(info.overview.as_deref(), Some("Space bounty hunters."));
        assert_eq!(info.rating, Some(8.6));
        assert_eq!(info.original_language, Some(8));
        assert_eq!(info.kind, EntryKind::Anime);
        assert!(info.external_ids.contains(&ExternalId {
            kind: ExternalIdKind::MyAnimeList,
            value: "1".into()
        }));
    }

    #[test]
    fn movies_have_one_dated_entry() {
        let info = map_media(media("MOVIE", None), vec![]);
        assert_eq!(info.kind, EntryKind::Movie);
        assert_eq!(info.episodes.len(), 1);
        assert_eq!(info.episodes[0].air_date.as_deref(), Some("1998-04-03"));
    }
}
