//! Sonarr's FileList indexer (`FileList/`): the tracker's JSON API, authenticated with the
//! user name and passkey (HTTP basic), searched by IMDb id and by name.

use serde_json::Value;

use crate::error::IndexerError;
use crate::feed::{Release, flags};
use crate::lenient;
use crate::request::{
    ACCEPT_JSON, Chain, Criteria, Pageable, Request, Search, Settings, escape_data_string,
    search_mode,
};

/// `GetRequest`: `{BaseUrl}/api.php?action=…&category=…`, nothing without categories.
fn request(settings: &Settings, action: &str, categories: &[i32], parameters: &str) -> Pageable {
    if categories.is_empty() {
        return Pageable::of(Vec::new(), 0);
    }
    let mut cats: Vec<String> = Vec::new();
    for c in categories.iter().map(i32::to_string) {
        if !cats.contains(&c) {
            cats.push(c);
        }
    }
    let url = format!(
        "{}/api.php?action={action}&category={}{parameters}",
        settings.base_url.trim_end_matches('/'),
        cats.join(",")
    );
    let mut r = Request::get(url, ACCEPT_JSON);
    r.basic_auth = Some((
        settings
            .username
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
        settings
            .passkey
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
    ));
    Pageable::of(vec![r], 0)
}

/// `GetRecentRequests`: the latest torrents of the standard and anime categories.
pub(crate) fn recent(settings: &Settings) -> Chain {
    let cats: Vec<i32> = settings
        .categories
        .iter()
        .chain(&settings.anime_categories)
        .copied()
        .collect();
    let mut chain = Chain::default();
    chain.add(request(settings, "latest-torrents", &cats, ""));
    chain
}

/// `AddImdbRequests`.
fn add_imdb(chain: &mut Chain, s: &Settings, c: &Criteria, cats: &[i32], parameters: &str) {
    if let Some(imdb) = c.ids.imdb.as_deref().filter(|i| !i.trim().is_empty()) {
        chain.add(request(
            s,
            "search-torrents",
            cats,
            &format!("&type=imdb&query={imdb}{parameters}"),
        ));
    }
}

/// `AddNameRequests`: each scene title.
fn add_names(chain: &mut Chain, s: &Settings, c: &Criteria, cats: &[i32], parameters: &str) {
    for title in &c.scene_titles {
        chain.add(request(
            s,
            "search-torrents",
            cats,
            &format!(
                "&type=name&query={}{parameters}",
                escape_data_string(title.trim())
            ),
        ));
    }
}

/// The standard and season searches: IMDb id, then names in a later tier.
fn standard(s: &Settings, c: &Criteria, parameters: &str) -> Chain {
    let mut chain = Chain::default();
    let cats = &s.categories;
    let mode = c.search_mode;
    if mode & search_mode::SEARCH_ID != 0 || mode == search_mode::DEFAULT {
        add_imdb(&mut chain, s, c, cats, parameters);
    }
    if mode & search_mode::SEARCH_TITLE != 0 {
        add_names(&mut chain, s, c, cats, parameters);
    }
    chain.add_tier();
    if mode == search_mode::DEFAULT {
        add_names(&mut chain, s, c, cats, parameters);
    }
    chain
}

/// `GetSearchRequests`. Daily and special searches are not supported, as upstream.
pub(crate) fn search(s: &Settings, c: &Criteria, search: &Search) -> Chain {
    match search {
        Search::SingleEpisode { season, episode } => {
            standard(s, c, &format!("&season={season}&episode={episode}"))
        }
        Search::Season { season } => standard(s, c, &format!("&season={season}")),
        Search::AnimeEpisode {
            absolute,
            season,
            episode,
        } => {
            // Absolute releases are numbered E01, others S01E01 (likely by IMDb's numbering);
            // the season/episode numbers are a fallback.
            let mut chain = Chain::default();
            let cats = &s.anime_categories;
            let absolute = format!("&season=0&episode={absolute}");
            let standard = format!("&season={season}&episode={episode}");
            add_imdb(&mut chain, s, c, cats, &absolute);
            chain.add_tier();
            add_imdb(&mut chain, s, c, cats, &standard);
            chain.add_tier();
            add_names(&mut chain, s, c, cats, &absolute);
            chain.add_tier();
            add_names(&mut chain, s, c, cats, &standard);
            chain
        }
        Search::AnimeSeason { season, .. } => {
            let mut chain = Chain::default();
            let cats = &s.anime_categories;
            let p = format!("&season={season}");
            add_imdb(&mut chain, s, c, cats, &p);
            chain.add_tier();
            add_names(&mut chain, s, c, cats, &p);
            chain
        }
        _ => Chain::default(),
    }
}

/// `FileListParser.ParseResponse` (the status and content type are checked by the client).
pub fn parse(body: &str, settings: &Settings) -> Result<Vec<Release>, IndexerError> {
    let results: Vec<Value> = lenient::parse(body)?;
    let base = settings.base_url.trim_end_matches('/');
    let passkey = settings.passkey.as_deref().unwrap_or_default().trim();
    let mut releases = Vec::with_capacity(results.len());
    for r in &results {
        let id = lenient::text(r.get("id")).unwrap_or_default();
        let seeders = lenient::int(r.get("seeders")).unwrap_or(0);
        let leechers = lenient::int(r.get("leechers")).unwrap_or(0);
        let mut indexer_flags = 0;
        if lenient::boolean(r.get("freeleech")) {
            indexer_flags |= flags::FREELEECH;
        }
        if lenient::boolean(r.get("internal")) {
            indexer_flags |= flags::INTERNAL;
        }
        let publish_date = lenient::date(r.get("upload_date")).ok_or_else(|| {
            IndexerError::Unexpected(format!("FileList torrent {id} has no valid upload date"))
        })?;
        releases.push(Release {
            guid: format!("FileList-{id}"),
            title: lenient::text(r.get("name")).unwrap_or_default(),
            size: lenient::int(r.get("size")).unwrap_or(0),
            download_url: Some(format!(
                "{base}/download.php?id={}&passkey={}",
                escape_data_string(&id),
                escape_data_string(passkey)
            )),
            info_url: Some(format!("{base}/details.php?id={}", escape_data_string(&id))),
            comment_url: None,
            publish_date,
            protocol: kitsunarr_domain::DownloadProtocol::Torrent,
            tvdb_id: 0,
            tv_rage_id: 0,
            imdb_id: lenient::text(r.get("imdb"))
                .filter(|i| i.len() > 2)
                .and_then(|i| lenient::imdb(&i)),
            languages: Vec::new(),
            indexer_flags,
            codec: None,
            container: None,
            info_hash: None,
            magnet_url: None,
            seeders: Some(seeders),
            peers: Some(leechers + seeders),
        });
    }
    Ok(releases)
}

/// `FileListParser`'s response checks: HTTP 200 with a JSON content type.
pub(crate) fn check_response(status: u16, content_type: &str) -> Result<(), IndexerError> {
    if status != 200 {
        return Err(IndexerError::Indexer(format!(
            "Unexpected response status {status} code from API request"
        )));
    }
    if !content_type.contains(ACCEPT_JSON) {
        return Err(IndexerError::Indexer(format!(
            "Unexpected response header '{content_type}' from indexer request, expected '{ACCEPT_JSON}'"
        )));
    }
    Ok(())
}
