//! Sonarr's BroadcasTheNet indexer (`BroadcastheNet/`): the tracker's JSON-RPC API
//! (`getTorrents`), searched by TheTVDB or TVRage id with release-name patterns, paged ten
//! pages of 100 deep, and read for RSS from just before the last torrent seen.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use kitsunarr_parser::netregex::NetRegex;
use serde_json::{Map, Value, json};

use crate::error::IndexerError;
use crate::feed::{Release, flags};
use crate::lenient;
use crate::request::{Chain, Criteria, Pageable, Request, Search, Settings, clean_scene_title};

/// `BroadcastheNet.PageSize`.
pub const PAGE_SIZE: u32 = 100;
/// `BroadcastheNetRequestGenerator.MaxPages`.
pub const MAX_PAGES: u32 = 10;
/// `JsonRpcRequestBuilder.JsonRpcHttpAccept`.
const ACCEPT_JSON_RPC: &str = "application/json-rpc, application/json";

static PROTOCOL: LazyLock<NetRegex> = LazyLock::new(|| NetRegex::compile("^https?:", false));

/// `CreateNextId`: an 8-character request id.
fn next_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = Utc::now().timestamp_subsec_nanos() as u64;
    format!("{:08x}", (t ^ n.wrapping_mul(0x9e37_79b9)) & 0xffff_ffff)
}

/// `GetPagedRequests`: `getTorrents(apiKey, query, 100, offset)` for each of the ten pages.
fn paged(settings: &Settings, query: &Map<String, Value>) -> Pageable {
    let api_key = settings.api_key.as_deref().unwrap_or_default();
    let requests = (0..MAX_PAGES)
        .map(|page| {
            let body = json!({
                "jsonrpc": "2.0",
                "method": "getTorrents",
                "params": [api_key, query, PAGE_SIZE, page * PAGE_SIZE],
                "id": next_id(),
            });
            Request::post_json(settings.base_url.clone(), ACCEPT_JSON_RPC, body.to_string())
        })
        .collect();
    Pageable::of(requests, PAGE_SIZE)
}

fn query(pairs: &[(&str, String)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::String(v.clone())))
        .collect()
}

/// The torrent id of a BroadcasTheNet release guid (`BTN-123`).
pub fn torrent_id(guid: &str) -> Option<i64> {
    guid.replace("BTN-", "").parse().ok()
}

/// `GetRecentRequests`: torrents from 100 before the last one seen, then (in a later tier)
/// those of the last day.
pub fn recent(settings: &Settings, last_torrent_id: Option<i64>) -> Chain {
    let mut chain = Chain::default();
    if let Some(id) = last_torrent_id.filter(|id| *id > 0) {
        chain.add(paged(
            settings,
            &query(&[("id", format!(">={}", id - 100))]),
        ));
    }
    chain.add_tier();
    chain.add(paged(settings, &query(&[("age", "<=86400".into())])));
    chain
}

/// `AddSeriesSearchParameters`: the TheTVDB id (positive), else the TVRage id.
fn series(c: &Criteria) -> Option<(&'static str, String)> {
    if c.ids.tvdb > 0 {
        Some(("tvdb", c.ids.tvdb.to_string()))
    } else if c.ids.tv_rage != 0 {
        Some(("tvrage", c.ids.tv_rage.to_string()))
    } else {
        None
    }
}

/// The second tier of daily searches: each episode by season and episode number.
fn daily_tier(
    settings: &Settings,
    mut chain: Chain,
    id: &(&'static str, String),
    episodes: &[(i32, i32)],
) -> Chain {
    chain.add_tier();
    for (season, episode) in episodes {
        chain.add(paged(
            settings,
            &query(&[
                id.clone(),
                ("category", "Episode".into()),
                ("name", format!("S{season:02}E{episode:02}")),
            ]),
        ));
    }
    chain
}

fn date_name(date: chrono::NaiveDate) -> String {
    date.format("%Y.%m.%d").to_string()
}

/// `GetSearchRequests`. Episode numbers below 100 are wildcarded so that three-digit numbers
/// match too. Daily searches add a second tier by the episodes' season and episode numbers.
pub(crate) fn search(settings: &Settings, c: &Criteria, search: &Search) -> Chain {
    let mut chain = Chain::default();
    let Some(id) = series(c) else {
        return chain;
    };
    let mut add = |category: &str, name: String| {
        chain.add(paged(
            settings,
            &query(&[id.clone(), ("category", category.into()), ("name", name)]),
        ));
    };
    match search {
        Search::SingleEpisode { season, episode } => add(
            "Episode",
            if (1..100).contains(episode) {
                format!("S{season:02}%E%{episode:02}%")
            } else {
                format!("S{season:02}%E{episode:02}%")
            },
        ),
        Search::Season { season } | Search::AnimeSeason { season, .. } => {
            add("Season", format!("Season {season}%"));
            // The season wildcard already matches three-digit episode numbers.
            add("Episode", format!("S{season:02}E%"));
        }
        Search::DailyEpisode { date, episodes } => {
            add("Episode", date_name(*date));
            return daily_tier(settings, chain, &id, episodes);
        }
        Search::DailySeason { year, episodes } => {
            add("Episode", format!("{year}%"));
            return daily_tier(settings, chain, &id, episodes);
        }
        Search::AnimeEpisode {
            season, episode, ..
        } => {
            add(
                "Episode",
                if (1..100).contains(episode) {
                    format!("S{season:02}E%{episode:02}")
                } else {
                    format!("S{season:02}E{episode:02}")
                },
            );
            add("Season", format!("Season {season}%"));
        }
        Search::Special { episode_titles, .. } => {
            for title in episode_titles.iter().filter(|t| !t.trim().is_empty()) {
                add("Episode", format!("%{}%", clean_scene_title(title)));
            }
        }
        Search::Movie { .. } | Search::CompleteSeries { .. } => {}
    }
    chain
}

/// `BroadcastheNetParser`'s status handling (the API's errors are returned with
/// `SuppressHttpError`).
pub(crate) fn check_response(
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), IndexerError> {
    match status {
        401 => {
            return Err(IndexerError::ApiKey(
                "API Key invalid or not authorized".into(),
            ));
        }
        404 => {
            return Err(IndexerError::Indexer(
                "Indexer API call returned NotFound, the Indexer API may have changed.".into(),
            ));
        }
        503 => {
            return Err(IndexerError::RequestLimit {
                retry_after_secs: None,
            });
        }
        200 => {}
        other => {
            return Err(IndexerError::Indexer(format!(
                "Indexer API call returned an unexpected StatusCode [{other}]"
            )));
        }
    }
    if content_type.contains("text/html") {
        return Err(IndexerError::Indexer(
            "Indexer responded with html content. Site is likely blocked or unavailable.".into(),
        ));
    }
    if body.to_lowercase().contains("call limit exceeded") {
        return Err(IndexerError::RequestLimit {
            retry_after_secs: None,
        });
    }
    if body == "Query execution was interrupted" {
        return Err(IndexerError::Indexer(
            "Indexer API returned an internal server error".into(),
        ));
    }
    Ok(())
}

/// `BroadcastheNetParser.ParseResponse`: links take the request's scheme.
pub fn parse(body: &str, request_url: &str) -> Result<Vec<Release>, IndexerError> {
    let doc: Value = lenient::parse(body)?;
    let error = doc.get("error").filter(|e| !e.is_null());
    let result = doc.get("result").filter(|r| !r.is_null());
    let (None, Some(result)) = (error, result) else {
        return Err(IndexerError::Indexer(format!(
            "Indexer API call returned an error [{}]",
            error.map(Value::to_string).unwrap_or_default()
        )));
    };
    let torrents = result.get("torrents").and_then(Value::as_object);
    let (Some(torrents), Some(results)) = (torrents, lenient::int(result.get("results"))) else {
        return Ok(Vec::new());
    };
    if results == 0 {
        return Ok(Vec::new());
    }
    let scheme = url::Url::parse(request_url)
        .map(|u| u.scheme().to_string())
        .unwrap_or_else(|_| "https".into());
    let protocol = format!("{scheme}:");
    let mut releases = Vec::with_capacity(torrents.len());
    for t in torrents.values() {
        let torrent_id = lenient::int(t.get("TorrentID")).unwrap_or(0);
        let group_id = lenient::int(t.get("GroupID")).unwrap_or(0);
        let seeders = lenient::int(t.get("Seeders"));
        let leechers = lenient::int(t.get("Leechers"));
        let origin = lenient::text(t.get("Origin")).unwrap_or_default();
        // BroadcasTheNet is freeleech throughout.
        let mut indexer_flags = flags::FREELEECH;
        match origin.to_uppercase().as_str() {
            "INTERNAL" => indexer_flags |= flags::INTERNAL,
            "SCENE" => indexer_flags |= flags::SCENE,
            _ => {}
        }
        if t.get("Tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| tags.iter().any(|v| v.as_str() == Some("Subtitles")))
        {
            indexer_flags |= flags::SUBTITLES;
        }
        let time = lenient::int(t.get("Time")).unwrap_or(0);
        releases.push(Release {
            guid: format!("BTN-{torrent_id}"),
            title: lenient::text(t.get("ReleaseName"))
                .unwrap_or_default()
                .replace('\\', ""),
            size: lenient::int(t.get("Size")).unwrap_or(0),
            download_url: lenient::text(t.get("DownloadURL"))
                .map(|u| PROTOCOL.replace(&u, &protocol)),
            info_url: Some(format!(
                "{protocol}//broadcasthe.net/torrents.php?id={group_id}&torrentid={torrent_id}"
            )),
            comment_url: None,
            publish_date: DateTime::from_timestamp(time, 0).unwrap_or_default(),
            protocol: kitsunarr_domain::DownloadProtocol::Torrent,
            tvdb_id: lenient::int(t.get("TvdbID"))
                .filter(|i| *i > 0)
                .unwrap_or(0),
            tv_rage_id: lenient::int(t.get("TvrageID"))
                .filter(|i| *i > 0)
                .unwrap_or(0),
            imdb_id: lenient::text(t.get("ImdbID"))
                .and_then(|i| i.parse::<i32>().ok())
                .filter(|i| *i > 0)
                .map(|i| format!("tt{i:07}")),
            languages: Vec::new(),
            indexer_flags,
            codec: lenient::text(t.get("Codec")),
            container: lenient::text(t.get("Container")),
            info_hash: lenient::text(t.get("InfoHash")),
            magnet_url: None,
            seeders,
            peers: match (leechers, seeders) {
                (Some(l), Some(s)) => Some(l + s),
                _ => None,
            },
        });
    }
    Ok(releases)
}
