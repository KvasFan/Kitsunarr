//! Request generation (`NewznabRequestGenerator.cs`, `IndexerPageableRequestChain.cs`,
//! `SearchCriteriaBase.cs`), shared by Newznab and Torznab as upstream does.

use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use kitsunarr_parser::netregex::NetRegex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::caps::Capabilities;

/// Pages requested per pageable request (`MaxPages`).
pub const MAX_PAGES: u32 = 30;

/// Indexer settings (`NewznabSettings` / `TorznabSettings`), read from the stored provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub base_url: String,
    pub api_path: String,
    pub api_key: Option<String>,
    pub categories: Vec<i32>,
    pub anime_categories: Vec<i32>,
    pub anime_standard_format_search: bool,
    pub additional_parameters: Option<String>,
    pub minimum_seeders: Option<i32>,
    /// Tanukarr's movie categories (default `[2000]`).
    pub movie_categories: Vec<i32>,
}

impl Settings {
    /// From the provider's `settings` map (Sonarr field names).
    pub fn from_map(map: &Map<String, Value>) -> Result<Self, String> {
        let base_url = map
            .get("baseUrl")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or("the indexer has no URL")?
            .trim()
            .to_string();
        let ints = |key: &str| -> Vec<i32> {
            map.get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_i64())
                        .map(|v| v as i32)
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(Self {
            base_url,
            api_path: map
                .get("apiPath")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("/api")
                .to_string(),
            api_key: map
                .get("apiKey")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            categories: ints("categories"),
            anime_categories: ints("animeCategories"),
            anime_standard_format_search: map
                .get("animeStandardFormatSearch")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            additional_parameters: map
                .get("additionalParameters")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            minimum_seeders: map
                .get("minimumSeeders")
                .and_then(Value::as_i64)
                .map(|v| v as i32),
            movie_categories: if map.contains_key("movieCategories") {
                ints("movieCategories")
            } else {
                vec![2000]
            },
        })
    }

    /// `{BaseUrl}{ApiPath}` without trailing slashes.
    pub fn api_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            self.api_path.trim_end_matches('/')
        )
    }

    /// The `t=caps` URL.
    pub fn caps_url(&self) -> String {
        let mut url = format!("{}?t=caps", self.api_url());
        if let Some(key) = &self.api_key {
            url.push_str("&apikey=");
            url.push_str(key);
        }
        url
    }

    /// Torznab warns about Jackett's "all" aggregate endpoint.
    pub fn is_jackett_all(&self) -> bool {
        let both = format!("{}{}", self.base_url, self.api_path).to_lowercase();
        both.contains("/torznab/all") || both.contains("/api/v2.0/indexers/all/results/torznab")
    }
}

/// Series identifiers sent to indexers (`0`/`None` when unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesIds {
    pub tvdb: i64,
    pub imdb: Option<String>,
    pub tv_rage: i64,
    pub tv_maze: i64,
    pub tmdb: i64,
}

/// Scene-mapping search modes (`SearchMode` flags).
pub mod search_mode {
    pub const DEFAULT: u8 = 0;
    pub const SEARCH_ID: u8 = 1;
    pub const SEARCH_TITLE: u8 = 2;
}

/// What is being searched for, with the series context (`SearchCriteriaBase`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Criteria {
    pub ids: SeriesIds,
    pub scene_titles: Vec<String>,
    pub search_mode: u8,
    /// `Series.SeriesType == Anime`.
    pub anime: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Search {
    SingleEpisode {
        season: i32,
        episode: i32,
    },
    Season {
        season: i32,
    },
    DailyEpisode {
        date: NaiveDate,
    },
    DailySeason {
        year: i32,
    },
    AnimeEpisode {
        absolute: i32,
        season: i32,
        episode: i32,
    },
    AnimeSeason {
        season: i32,
        season_scene_titles: Vec<String>,
    },
    Special {
        episode_query_titles: Vec<String>,
    },
    /// Tanukarr's `MovieSearch`: "Title Year" text searches on the movie categories (and the
    /// anime categories for anime films).
    Movie {
        query_titles: Vec<String>,
    },
    /// Tanukarr's complete-series search: the series by id, then text queries for its titles
    /// and for packs (`Title S01-S05`, `Title complete`, season packs).
    CompleteSeries {
        query_titles: Vec<String>,
        pack_query_titles: Vec<String>,
    },
}

/// A paged request: the URL without paging plus the criteria parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pageable {
    prefix: String,
    parameters: String,
    page_size: u32,
    /// Empty category list: upstream yields no requests for it.
    empty: bool,
}

impl Pageable {
    /// Page URLs, `MAX_PAGES` of them (one when the page size is 0).
    pub fn pages(&self) -> Vec<String> {
        if self.empty {
            return Vec::new();
        }
        if self.page_size == 0 {
            return vec![format!("{}{}", self.prefix, self.parameters)];
        }
        (0..MAX_PAGES)
            .map(|page| {
                format!(
                    "{}&offset={}&limit={}{}",
                    self.prefix,
                    page * self.page_size,
                    self.page_size,
                    self.parameters
                )
            })
            .collect()
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
    }
}

/// Requests in tiers: a later tier runs only when earlier ones found nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub tiers: Vec<Vec<Pageable>>,
}

impl Default for Chain {
    fn default() -> Self {
        Self {
            tiers: vec![Vec::new()],
        }
    }
}

impl Chain {
    fn add(&mut self, p: Pageable) {
        if let Some(last) = self.tiers.last_mut() {
            last.push(p);
        }
    }

    /// Starts a new tier unless the current one is empty.
    fn add_tier(&mut self) {
        if self.tiers.last().is_some_and(|t| !t.is_empty()) {
            self.tiers.push(Vec::new());
        }
    }

    pub fn all(&self) -> impl Iterator<Item = &Pageable> {
        self.tiers.iter().flatten()
    }
}

static BEGINNING_THE: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"^the\s", true));
static SPECIAL_CHARACTER: LazyLock<NetRegex> =
    LazyLock::new(|| NetRegex::compile(r"['.`´‘’]", true));
static NON_WORD: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"[\W]", true));
static PLUSES: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"\+{2,}", false));

/// `GetCleanSceneTitle`: `The Show's & Co.` → `Shows+and+Co`.
pub fn clean_scene_title(title: &str) -> String {
    let t = BEGINNING_THE.replace(title, "");
    let t = t.replace('&', "and");
    let t = SPECIAL_CHARACTER.replace(&t, "");
    let t = NON_WORD.replace(&t, "+");
    let t = PLUSES.replace(&t, "+");
    let t = kitsunarr_parser::remove_diacritics(&t);
    t.trim_matches(['+', ' ']).to_string()
}

fn distinct(v: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

fn clean_titles(titles: &[String]) -> Vec<String> {
    distinct(titles.iter().map(|t| clean_scene_title(t)))
}

fn all_titles(titles: &[String]) -> Vec<String> {
    distinct(titles.iter().cloned().chain(clean_titles(titles)))
}

/// `Uri.EscapeDataString`: everything but RFC 3986 unreserved characters is %-encoded.
pub fn escape_data_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `HttpUtility.UrlEncode`: spaces become `+`, lower-case %-escapes.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'*' | b'(' | b')')
        {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    out
}

/// `NewsnabifyTitle`: `+` → space, then escaped.
fn newznabify_title(title: &str) -> String {
    escape_data_string(&title.replace('+', " "))
}

/// `NewznabifySeasonNumber`: season 0 is sent as `00` (NNTMux workaround).
fn newznabify_season(season: i32) -> String {
    if season == 0 {
        "00".into()
    } else {
        season.to_string()
    }
}

/// Builds request chains for one indexer.
pub struct Generator<'a> {
    pub settings: &'a Settings,
    pub caps: &'a Capabilities,
    pub page_size: u32,
    /// Nyaa searches by title through its RSS feed instead of the Newznab API.
    pub dialect: crate::Dialect,
}

impl Generator<'_> {
    fn paged(&self, categories: &[i32], search_type: &str, parameters: String) -> Pageable {
        let cats = distinct(categories.iter().map(|c| c.to_string())).join(",");
        let mut prefix = format!(
            "{}?t={search_type}&cat={cats}&extended=1{}",
            self.settings.api_url(),
            self.settings.additional_parameters.as_deref().unwrap_or("")
        );
        if let Some(key) = &self.settings.api_key {
            prefix.push_str("&apikey=");
            prefix.push_str(key);
        }
        Pageable {
            prefix,
            parameters,
            page_size: self.page_size,
            empty: categories.is_empty(),
        }
    }

    fn search_categories(&self, c: &Criteria) -> &[i32] {
        if c.anime {
            &self.settings.anime_categories
        } else {
            &self.settings.categories
        }
    }

    /// `AddTvIdPageableRequests`.
    fn add_id_requests(
        &self,
        chain: &mut Chain,
        categories: &[i32],
        c: &Criteria,
        parameters: &str,
    ) {
        let ids = &c.ids;
        let tvdb = self.caps.supports_tv_param("tvdbid") && ids.tvdb > 0;
        let imdb = self.caps.supports_tv_param("imdbid")
            && ids.imdb.as_deref().is_some_and(|i| !i.trim().is_empty());
        let rage = self.caps.supports_tv_param("rid") && ids.tv_rage > 0;
        let maze = self.caps.supports_tv_param("tvmazeid") && ids.tv_maze > 0;
        let tmdb = self.caps.supports_tv_param("tmdbid") && ids.tmdb > 0;
        let imdb_value = ids.imdb.clone().unwrap_or_default();
        if self.caps.supports_aggregate_id_search && (tvdb || rage || maze || tmdb) {
            let mut q = String::new();
            if tvdb {
                q.push_str(&format!("&tvdbid={}", ids.tvdb));
            }
            if imdb {
                q.push_str(&format!("&imdbid={imdb_value}"));
            }
            if rage {
                q.push_str(&format!("&rid={}", ids.tv_rage));
            }
            if maze {
                q.push_str(&format!("&tvmazeid={}", ids.tv_maze));
            }
            if tmdb {
                q.push_str(&format!("&tmdbid={}", ids.tmdb));
            }
            chain.add(self.paged(categories, "tvsearch", q + parameters));
        } else if tvdb {
            chain.add(self.paged(
                categories,
                "tvsearch",
                format!("&tvdbid={}{parameters}", ids.tvdb),
            ));
        } else if imdb {
            chain.add(self.paged(
                categories,
                "tvsearch",
                format!("&imdbid={imdb_value}{parameters}"),
            ));
        } else if rage {
            chain.add(self.paged(
                categories,
                "tvsearch",
                format!("&rid={}{parameters}", ids.tv_rage),
            ));
        } else if maze {
            chain.add(self.paged(
                categories,
                "tvsearch",
                format!("&tvmazeid={}{parameters}", ids.tv_maze),
            ));
        } else if tmdb {
            chain.add(self.paged(
                categories,
                "tvsearch",
                format!("&tmdbid={}{parameters}", ids.tmdb),
            ));
        }
    }

    /// `AddTitlePageableRequests` (always the standard categories, as upstream).
    fn add_title_requests(&self, chain: &mut Chain, c: &Criteria, parameters: &str) {
        let cats = &self.settings.categories;
        if self.caps.supports_tv_title_search() {
            for title in &c.scene_titles {
                chain.add(self.paged(
                    cats,
                    "tvsearch",
                    format!("&title={}{parameters}", escape_data_string(title)),
                ));
            }
        } else if self.caps.supports_tv_query_search() {
            let titles = if self.caps.tv_text_search_engine == "raw" {
                all_titles(&c.scene_titles)
            } else {
                clean_titles(&c.scene_titles)
            };
            for title in titles {
                chain.add(self.paged(
                    cats,
                    "tvsearch",
                    format!("&q={}{parameters}", newznabify_title(&title)),
                ));
            }
        }
    }

    /// Tiered ID/title requests used by the standard and daily searches.
    fn standard(&self, c: &Criteria, categories: &[i32], parameters: &str) -> Chain {
        let mut chain = Chain::default();
        let mode = c.search_mode;
        if mode & search_mode::SEARCH_ID != 0 || mode == search_mode::DEFAULT {
            self.add_id_requests(&mut chain, categories, c, parameters);
        }
        if mode & search_mode::SEARCH_TITLE != 0 {
            self.add_title_requests(&mut chain, c, parameters);
        }
        chain.add_tier();
        if mode == search_mode::DEFAULT {
            self.add_title_requests(&mut chain, c, parameters);
        }
        chain
    }

    /// `GetRecentRequests` (RSS).
    pub fn recent(&self) -> Chain {
        if self.dialect == crate::Dialect::Nyaa {
            let mut chain = Chain::default();
            chain.add(self.nyaa_request(None));
            return chain;
        }
        let mut chain = Chain::default();
        if self.caps.tv_search_params.is_some() {
            let cats: Vec<i32> = self
                .settings
                .categories
                .iter()
                .chain(&self.settings.anime_categories)
                .copied()
                .collect();
            chain.add(self.paged(&cats, "tvsearch", String::new()));
        } else if self.caps.search_params.is_some() {
            chain.add(self.paged(&self.settings.anime_categories, "search", String::new()));
        }
        // Tanukarr: TV-search APIs may leave films out even with a movie category, so films
        // are read with a generic search of their own.
        if self.caps.search_params.is_some() && !self.settings.movie_categories.is_empty() {
            chain.add(self.paged(&self.settings.movie_categories, "search", String::new()));
        }
        chain
    }

    /// `GetSearchRequests` for each criteria type.
    /// `NyaaRequestGenerator.GetPagedRequests`: `{base}/?page=rss{additional}&term=…`.
    fn nyaa_request(&self, term: Option<String>) -> Pageable {
        let mut prefix = format!(
            "{}/?page=rss{}",
            self.settings.base_url.trim_end_matches('/'),
            self.settings.additional_parameters.as_deref().unwrap_or("")
        );
        if let Some(t) = term {
            prefix.push_str("&term=");
            prefix.push_str(&t);
        }
        Pageable {
            prefix,
            parameters: String::new(),
            page_size: 0,
            empty: false,
        }
    }

    /// `NyaaRequestGenerator`'s searches: titles (escaped, spaces as `+`) with the absolute
    /// number for anime episodes, `sXXeYY`/`sXX` with "anime standard format search". Scene
    /// and query titles arrive cleaned, with `+` between words.
    fn nyaa_search(&self, c: &Criteria, search: &Search) -> Chain {
        let q = |t: &str| url_encode(&t.replace('+', " "));
        let mut chain = Chain::default();
        let standard = self.settings.anime_standard_format_search;
        let mut titles: Vec<String> = Vec::new();
        match search {
            Search::SingleEpisode { season, episode }
                if standard && *season > 0 && *episode > 0 =>
            {
                for t in &c.scene_titles {
                    titles.push(format!("{}+s{season:02}e{episode:02}", q(t)));
                }
            }
            Search::Season { season } if standard && *season > 0 => {
                for t in &c.scene_titles {
                    titles.push(format!("{}+s{season:02}", q(t)));
                }
            }
            Search::AnimeEpisode {
                absolute,
                season,
                episode,
            } => {
                for t in &c.scene_titles {
                    if *absolute > 0 {
                        titles.push(format!("{}+{absolute}", q(t)));
                        if *absolute < 10 {
                            titles.push(format!("{}+{absolute:02}", q(t)));
                        }
                    }
                    if standard && *season > 0 && *episode > 0 {
                        titles.push(format!("{}+s{season:02}e{episode:02}", q(t)));
                    }
                }
            }
            Search::AnimeSeason {
                season,
                season_scene_titles,
            } => {
                for t in &c.scene_titles {
                    if season_scene_titles
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(t))
                    {
                        titles.push(q(t));
                    } else if standard && *season > 0 {
                        titles.push(format!("{}+s{season:02}", q(t)));
                    }
                }
            }
            Search::Special {
                episode_query_titles,
            } => titles.extend(episode_query_titles.iter().map(|t| q(t))),
            Search::Movie { query_titles } => titles.extend(query_titles.iter().map(|t| q(t))),
            Search::CompleteSeries {
                query_titles,
                pack_query_titles,
            } => titles.extend(query_titles.iter().chain(pack_query_titles).map(|t| q(t))),
            _ => {}
        }
        let mut seen = Vec::new();
        for t in titles {
            if !seen.iter().any(|s: &String| s.eq_ignore_ascii_case(&t)) {
                seen.push(t.clone());
                chain.add(self.nyaa_request(Some(t)));
            }
        }
        chain
    }

    pub fn search(&self, c: &Criteria, search: &Search) -> Chain {
        if self.dialect == crate::Dialect::Nyaa {
            return self.nyaa_search(c, search);
        }
        let caps = self.caps;
        let text_or_id = caps.supports_tv_text_searches() || caps.supports_tv_id_searches();
        match search {
            Search::SingleEpisode { season, episode } => {
                if !caps.supports_episode_search() || !text_or_id {
                    return Chain::default();
                }
                let p = format!("&season={}&ep={episode}", newznabify_season(*season));
                self.standard(c, self.search_categories(c), &p)
            }
            Search::Season { season } => {
                if !caps.supports_season_search() || !text_or_id {
                    return Chain::default();
                }
                let p = format!("&season={}", newznabify_season(*season));
                self.standard(c, &self.settings.categories, &p)
            }
            Search::DailyEpisode { date } => {
                if !caps.supports_episode_search() || !text_or_id {
                    return Chain::default();
                }
                let p = format!(
                    "&season={:04}&ep={:02}/{:02}",
                    date.year(),
                    date.month(),
                    date.day()
                );
                self.standard(c, &self.settings.categories, &p)
            }
            Search::DailySeason { year } => {
                if !caps.supports_episode_search() || !text_or_id {
                    return Chain::default();
                }
                self.standard(c, &self.settings.categories, &format!("&season={year}"))
            }
            Search::AnimeEpisode {
                absolute,
                season,
                episode,
            } => {
                let mut chain = Chain::default();
                if !caps.supports_search() {
                    return chain;
                }
                let cats = &self.settings.anime_categories;
                self.add_id_requests(&mut chain, cats, c, &format!("&q={absolute:02}"));
                let standard =
                    self.settings.anime_standard_format_search && *season > 0 && *episode > 0;
                let se = format!("&season={}&ep={episode}", newznabify_season(*season));
                if standard && caps.supports_episode_search() {
                    self.add_id_requests(&mut chain, cats, c, &se);
                }
                let titles = if caps.text_search_engine == "raw" {
                    all_titles(&c.scene_titles)
                } else {
                    clean_titles(&c.scene_titles)
                };
                for title in titles {
                    chain.add(self.paged(
                        cats,
                        "search",
                        format!("&q={}+{absolute:02}", newznabify_title(&title)),
                    ));
                    if standard && caps.supports_episode_search() {
                        chain.add(self.paged(
                            cats,
                            "tvsearch",
                            format!("&q={}{se}", newznabify_title(&title)),
                        ));
                    }
                }
                chain
            }
            Search::AnimeSeason {
                season,
                season_scene_titles,
            } => {
                let mut chain = Chain::default();
                if !caps.supports_search() || *season <= 0 {
                    return chain;
                }
                let raw = caps.text_search_engine == "raw";
                let titles = if raw {
                    all_titles(&c.scene_titles)
                } else {
                    clean_titles(&c.scene_titles)
                };
                let season_titles = if raw {
                    all_titles(season_scene_titles)
                } else {
                    clean_titles(season_scene_titles)
                };
                let cats = &self.settings.anime_categories;
                let p = format!("&season={}", newznabify_season(*season));
                if self.settings.anime_standard_format_search {
                    self.add_id_requests(&mut chain, cats, c, &p);
                }
                for title in titles {
                    if season_titles
                        .iter()
                        .any(|s| s.to_lowercase() == title.to_lowercase())
                    {
                        chain.add(self.paged(
                            cats,
                            "search",
                            format!("&q={}", newznabify_title(&title)),
                        ));
                    } else if self.settings.anime_standard_format_search {
                        chain.add(self.paged(
                            cats,
                            "tvsearch",
                            format!("&q={}{p}", newznabify_title(&title)),
                        ));
                    }
                }
                chain
            }
            Search::CompleteSeries {
                query_titles,
                pack_query_titles,
            } => {
                let mut chain = Chain::default();
                let cats = self.search_categories(c).to_vec();
                self.add_id_requests(&mut chain, &cats, c, "");
                // ID results can be single episodes, so title and pack queries share their tier.
                if caps.supports_search() {
                    let mut seen = std::collections::HashSet::new();
                    for q in query_titles.iter().chain(pack_query_titles) {
                        if seen.insert(q.to_lowercase()) {
                            chain.add(self.paged(&cats, "search", format!("&q={}", url_encode(q))));
                        }
                    }
                } else {
                    self.add_title_requests(&mut chain, c, "");
                }
                chain
            }
            Search::Movie { query_titles } => {
                let mut chain = Chain::default();
                if caps.supports_search() {
                    let mut cats = self.settings.movie_categories.clone();
                    if c.anime {
                        for a in &self.settings.anime_categories {
                            if !cats.contains(a) {
                                cats.push(*a);
                            }
                        }
                    }
                    for title in query_titles {
                        let q = url_encode(&title.replace('+', " "));
                        chain.add(self.paged(&cats, "search", format!("&q={q}")));
                    }
                }
                chain
            }
            Search::Special {
                episode_query_titles,
            } => {
                let mut chain = Chain::default();
                if caps.supports_search() {
                    let cats = self.search_categories(c).to_vec();
                    for title in episode_query_titles {
                        let q = url_encode(&title.replace('+', " "));
                        chain.add(self.paged(&cats, "search", format!("&q={q}")));
                    }
                }
                chain
            }
        }
    }
}
