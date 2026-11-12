//! Sonarr's Fanzub indexer (`Fanzub/FanzubRequestGenerator.cs`): a usenet anime RSS feed
//! searched by quoted title alternatives. Fanzub itself has been offline for years; the
//! indexer is kept, as upstream keeps it, for feeds that copied its interface.
//!
//! Its feed is parsed by [`crate::torrent_rss::RssParser::fanzub`].

use std::sync::LazyLock;

use kitsunarr_parser::netregex::NetRegex;

use crate::request::{
    ACCEPT_RSS, Chain, Criteria, Pageable, Request, Search, Settings, clean_titles,
};

/// `FanzubRequestGenerator.PageSize`.
const PAGE_SIZE: u32 = 100;

static REMOVE_CHARACTERS: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile(r"[!?`]", false));

/// `GetPagedRequests`: `{BaseUrl}?cat=anime&max=100[&q=…]`.
fn paged(settings: &Settings, query: Option<String>) -> Pageable {
    let mut url = format!("{}?cat=anime&max={PAGE_SIZE}", settings.base_url);
    if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
        url.push_str("&q=");
        url.push_str(&q);
    }
    // Fanzub does not page (`PageSize` 0): one request.
    Pageable::of(vec![Request::get(url, ACCEPT_RSS)], 0)
}

/// `GetRecentRequests`.
pub(crate) fn recent(settings: &Settings) -> Chain {
    let mut chain = Chain::default();
    chain.add(paged(settings, None));
    chain
}

fn clean_title(title: &str) -> String {
    REMOVE_CHARACTERS.replace(title, "")
}

fn quoted(formats: [String; 2]) -> impl Iterator<Item = String> {
    formats.into_iter().map(|f| format!("\"{f}\""))
}

/// `GetSeasonSearchStrings`.
fn season_strings(title: &str, season: i32) -> impl Iterator<Item = String> {
    let t = clean_title(title);
    quoted([
        format!("{t}%20S{season:02}"),
        format!("{t}%20-%20S{season:02}"),
    ])
}

/// `GetTitleSearchStrings` with an absolute number.
fn absolute_strings(title: &str, absolute: i32) -> impl Iterator<Item = String> {
    let t = clean_title(title);
    quoted([
        format!("{t}%20{absolute:02}"),
        format!("{t}%20-%20{absolute:02}"),
    ])
}

/// `GetTitleSearchStrings` with a season and episode.
fn episode_strings(title: &str, season: i32, episode: i32) -> impl Iterator<Item = String> {
    let t = clean_title(title);
    quoted([
        format!("{t}%20S{season:02}E{episode:02}"),
        format!("{t}%20-%20S{season:02}E{episode:02}"),
    ])
}

/// `GetSearchRequests`: season searches only with "anime standard format search", anime
/// episodes by absolute number (and season/episode with that option); nothing else.
pub(crate) fn search(settings: &Settings, c: &Criteria, search: &Search) -> Chain {
    let mut chain = Chain::default();
    let titles = clean_titles(&c.scene_titles);
    let standard = settings.anime_standard_format_search;
    let terms: Vec<String> = match search {
        Search::Season { season } | Search::AnimeSeason { season, .. }
            if standard && *season > 0 =>
        {
            titles
                .iter()
                .flat_map(|t| season_strings(t, *season))
                .collect()
        }
        Search::AnimeEpisode {
            absolute,
            season,
            episode,
        } => {
            let mut terms: Vec<String> = titles
                .iter()
                .flat_map(|t| absolute_strings(t, *absolute))
                .collect();
            if standard && *season > 0 && *episode > 0 {
                terms.extend(
                    titles
                        .iter()
                        .flat_map(|t| episode_strings(t, *season, *episode)),
                );
            }
            terms
        }
        _ => return chain,
    };
    chain.add(paged(settings, Some(terms.join("|"))));
    chain
}
