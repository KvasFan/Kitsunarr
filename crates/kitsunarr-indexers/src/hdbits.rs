//! Sonarr's HDBits indexer (`HDBits/`): a JSON API queried with POSTed queries, searched by
//! TheTVDB id.

use serde_json::{Map, Value, json};

use crate::error::IndexerError;
use crate::feed::{Release, flags};
use crate::lenient;
use crate::request::{ACCEPT_JSON, Chain, Criteria, Pageable, Request, Search, Settings};

/// `TorrentQuery.Limit`.
const LIMIT: u32 = 100;

/// `HDBits.StatusCode` names, by value.
const STATUS: &[&str] = &[
    "Success",
    "Failure",
    "SslRequired",
    "JsonMalformed",
    "AuthDataMissing",
    "AuthFailed",
    "MissingRequiredParameters",
    "InvalidParameter",
    "ImdbImportFail",
    "ImdbTvNotAllowed",
];

/// `GetRequest`: the query POSTed to `/api/torrents` with the credentials and filters.
fn request(settings: &Settings, mut query: Map<String, Value>) -> Pageable {
    query.insert(
        "username".into(),
        json!(settings.username.as_deref().unwrap_or_default()),
    );
    query.insert(
        "passkey".into(),
        json!(settings.api_key.as_deref().unwrap_or_default()),
    );
    query.insert("category".into(), json!(settings.categories));
    query.insert("codec".into(), json!(settings.codecs));
    query.insert("medium".into(), json!(settings.mediums));
    query.insert("limit".into(), json!(LIMIT));
    let url = format!("{}/api/torrents", settings.base_url.trim_end_matches('/'));
    let body = Value::Object(query).to_string();
    // One request per query: HDBits is not paged.
    Pageable::of(vec![Request::post_json(url, ACCEPT_JSON, body)], 0)
}

/// `GetRecentRequests`: the newest torrents of the configured categories.
pub(crate) fn recent(settings: &Settings) -> Chain {
    let mut chain = Chain::default();
    chain.add(request(settings, Map::new()));
    chain
}

/// `TryAddSearchParameters`: searches need a (positive) TheTVDB id.
fn tvdb_query(
    c: &Criteria,
    season: Option<i32>,
    episode: Option<i32>,
) -> Option<Map<String, Value>> {
    if c.ids.tvdb <= 0 {
        return None;
    }
    let mut tvdb = Map::new();
    tvdb.insert("id".into(), json!(c.ids.tvdb));
    if let Some(s) = season {
        tvdb.insert("season".into(), json!(s));
    }
    if let Some(e) = episode {
        tvdb.insert("episode".into(), json!(e));
    }
    let mut query = Map::new();
    query.insert("tvdb".into(), Value::Object(tvdb));
    Some(query)
}

/// `GetSearchRequests`. Upstream's anime-episode search builds its queries without adding
/// them, so it requests nothing; specials are not searched either.
pub(crate) fn search(settings: &Settings, c: &Criteria, search: &Search) -> Chain {
    let mut chain = Chain::default();
    let query = match search {
        Search::SingleEpisode { season, episode } => tvdb_query(c, Some(*season), Some(*episode)),
        Search::Season { season } | Search::AnimeSeason { season, .. } => {
            tvdb_query(c, Some(*season), None)
        }
        Search::DailyEpisode { date, .. } => tvdb_query(c, None, None).map(|mut q| {
            q.insert("search".into(), json!(date.format("%Y-%m-%d").to_string()));
            q
        }),
        Search::DailySeason { year, .. } => tvdb_query(c, None, None).map(|mut q| {
            q.insert("search".into(), json!(format!("{year}-")));
            q
        }),
        _ => None,
    };
    if let Some(q) = query {
        chain.add(request(settings, q));
    }
    chain
}

/// `HDBitsParser.ParseResponse` (the HTTP status is checked by the client).
pub fn parse(body: &str, settings: &Settings) -> Result<Vec<Release>, IndexerError> {
    let doc: Value = lenient::parse(body)?;
    let status = lenient::int(doc.get("status"))
        .ok_or_else(|| IndexerError::Unexpected("HDBits response without a status".into()))?;
    if status != 0 {
        let name = usize::try_from(status)
            .ok()
            .and_then(|s| STATUS.get(s))
            .map_or_else(|| status.to_string(), |s| (*s).to_string());
        let message = lenient::text(doc.get("message")).unwrap_or_default();
        return Err(IndexerError::Indexer(format!(
            "HDBits API request returned status code {name}: {message}"
        )));
    }
    let Some(data) = doc.get("data").and_then(Value::as_array) else {
        return Err(IndexerError::Indexer(
            "Indexer API call response missing result data".into(),
        ));
    };
    let base = settings.base_url.trim_end_matches('/');
    let passkey = settings.api_key.as_deref().unwrap_or_default();
    let mut releases = Vec::with_capacity(data.len());
    for r in data {
        let id = lenient::text(r.get("id")).unwrap_or_default();
        let seeders = lenient::int(r.get("seeders")).unwrap_or(0);
        let leechers = lenient::int(r.get("leechers")).unwrap_or(0);
        let mut indexer_flags = 0;
        if lenient::text(r.get("freeleech")).as_deref() == Some("yes") {
            indexer_flags |= flags::FREELEECH;
        }
        if lenient::int(r.get("type_origin")) == Some(1) {
            indexer_flags |= flags::INTERNAL;
        }
        let publish_date = lenient::date(r.get("added")).ok_or_else(|| {
            IndexerError::Unexpected(format!("HDBits torrent {id} has no valid date"))
        })?;
        releases.push(Release {
            guid: format!("HDBits-{id}"),
            title: lenient::text(r.get("name")).unwrap_or_default(),
            size: lenient::int(r.get("size")).unwrap_or(0),
            download_url: Some(format!(
                "{base}/download.php?id={}&passkey={}",
                crate::request::escape_data_string(&id),
                crate::request::escape_data_string(passkey)
            )),
            info_url: Some(format!(
                "{base}/details.php?id={}",
                crate::request::escape_data_string(&id)
            )),
            comment_url: None,
            publish_date,
            protocol: kitsunarr_domain::DownloadProtocol::Torrent,
            tvdb_id: lenient::int(r.get("tvdb").and_then(|t| t.get("id"))).unwrap_or(0),
            tv_rage_id: 0,
            imdb_id: None,
            languages: Vec::new(),
            indexer_flags,
            codec: None,
            container: None,
            info_hash: lenient::text(r.get("hash")),
            magnet_url: None,
            seeders: Some(seeders),
            peers: Some(leechers + seeders),
        });
    }
    Ok(releases)
}
