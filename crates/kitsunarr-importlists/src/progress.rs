//! Watched-progress writers (Tanukarr's `AniListProgressWriter`, `MyAnimeListProgressWriter`):
//! advance the episode count of an entry already on the user's own list. They never create
//! entries, never lower a count, and send nothing but the progress.

use serde_json::{Map, Value, json};

use crate::{ListError, check_status, connection_error, text};

/// Why a write did not happen.
#[derive(Debug, thiserror::Error)]
pub enum ProgressError {
    /// Retried after the usual back-off.
    #[error(transparent)]
    List(#[from] ListError),
    /// Retried after a day (Tanukarr's `ImportListProgressUnavailableException`): the account,
    /// entry or token is not usable.
    #[error("{0}")]
    Unavailable(String),
}

type Result<T> = std::result::Result<T, ProgressError>;

fn unavailable(m: impl Into<String>) -> ProgressError {
    ProgressError::Unavailable(m.into())
}

/// Sets the progress of AniList media `media_id` to at least `progress` on the list of the
/// settings' user; returns the progress AniList confirms.
pub async fn anilist(
    http: &reqwest::Client,
    s: &Map<String, Value>,
    media_id: i64,
    progress: i64,
) -> Result<i64> {
    let token = text(s, "accessToken")
        .ok_or_else(|| unavailable("AniList progress needs an access token"))?;
    let user =
        text(s, "username").ok_or_else(|| unavailable("the AniList list has no username"))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://graphql.anilist.co".into());
    let call = |query: &'static str, variables: Value| {
        let (http, base, token) = (http.clone(), base.clone(), token.clone());
        async move {
            let resp = http
                .post(&base)
                .bearer_auth(token)
                .json(&json!({"query": query, "variables": variables}))
                .send()
                .await
                .map_err(connection_error)?;
            check_status(&resp)?;
            let v: Value = resp
                .json()
                .await
                .map_err(|_| ListError::Invalid("AniList did not answer JSON".into()))?;
            if v["errors"].as_array().is_some_and(|e| !e.is_empty()) {
                return Err(ListError::Invalid(format!(
                    "AniList returned errors: {}",
                    v["errors"]
                )));
            }
            Ok::<_, ListError>(v["data"].clone())
        }
    };
    // Own account only: the token's viewer must be the list's user.
    let who = call(
        "query ($name: String) { Viewer { id name } User(name: $name) { id name } }",
        json!({"name": user}),
    )
    .await?;
    let viewer = who["Viewer"]["id"].as_i64();
    if viewer.is_none() || viewer != who["User"]["id"].as_i64() {
        return Err(unavailable(
            "the AniList token belongs to another account than the list",
        ));
    }
    let entry = call(
        "query ($media: Int, $user: Int) { MediaList(mediaId: $media, userId: $user, type: ANIME) { id userId mediaId progress media { episodes type } } }",
        json!({"media": media_id, "user": viewer}),
    )
    .await?;
    let e = &entry["MediaList"];
    let Some(entry_id) = e["id"].as_i64() else {
        return Err(unavailable("the title is not on the AniList list"));
    };
    let remote = e["progress"].as_i64().unwrap_or(0);
    if remote >= progress {
        return Ok(remote);
    }
    if e["media"]["episodes"]
        .as_i64()
        .is_some_and(|n| progress > n)
    {
        return Err(unavailable(
            "the progress exceeds the title's episode count",
        ));
    }
    let saved = call(
        "mutation ($id: Int, $progress: Int) { SaveMediaListEntry(id: $id, progress: $progress) { id userId mediaId progress } }",
        json!({"id": entry_id, "progress": progress}),
    )
    .await?;
    let r = &saved["SaveMediaListEntry"];
    if r["id"].as_i64() != Some(entry_id)
        || r["mediaId"].as_i64() != Some(media_id)
        || r["progress"].as_i64().unwrap_or(0) < progress
    {
        return Err(ProgressError::List(ListError::Invalid(
            "AniList did not confirm the progress".into(),
        )));
    }
    Ok(r["progress"].as_i64().unwrap_or(progress))
}

/// Sets the number of watched episodes of MyAnimeList anime `anime_id` to at least
/// `progress`; returns what MyAnimeList confirms.
pub async fn myanimelist(
    http: &reqwest::Client,
    s: &Map<String, Value>,
    anime_id: i64,
    progress: i64,
) -> Result<i64> {
    let token =
        text(s, "accessToken").ok_or_else(|| unavailable("MyAnimeList progress needs a token"))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.myanimelist.net/v2".into());
    let base = base.trim_end_matches('/').to_string();
    let get = |path: String| {
        let (http, token) = (http.clone(), token.clone());
        async move {
            let resp = http
                .get(path)
                .bearer_auth(token)
                .send()
                .await
                .map_err(connection_error)?;
            check_status(&resp)?;
            resp.json::<Value>()
                .await
                .map_err(|_| ListError::Invalid("MyAnimeList did not answer JSON".into()))
        }
    };
    let me = get(format!("{base}/users/@me")).await?;
    if let Some(user) = text(s, "username").filter(|u| u != "@me")
        && me["name"].as_str() != Some(user.as_str())
    {
        return Err(unavailable(
            "the MyAnimeList token belongs to another account than the list",
        ));
    }
    let anime = get(format!(
        "{base}/anime/{anime_id}?fields=num_episodes,my_list_status{{num_episodes_watched}}"
    ))
    .await?;
    let Some(status) = anime.get("my_list_status").filter(|v| v.is_object()) else {
        return Err(unavailable("the title is not on the MyAnimeList list"));
    };
    let remote = status["num_episodes_watched"].as_i64().unwrap_or(0);
    if remote >= progress {
        return Ok(remote);
    }
    if anime["num_episodes"]
        .as_i64()
        .is_some_and(|n| n > 0 && progress > n)
    {
        return Err(unavailable(
            "the progress exceeds the title's episode count",
        ));
    }
    let resp = http
        .patch(format!("{base}/anime/{anime_id}/my_list_status"))
        .bearer_auth(&token)
        .form(&[("num_watched_episodes", progress.to_string())])
        .send()
        .await
        .map_err(connection_error)?;
    check_status(&resp)?;
    let v: Value = resp
        .json()
        .await
        .map_err(|_| ListError::Invalid("MyAnimeList did not answer JSON".into()))?;
    let confirmed = v["num_episodes_watched"].as_i64().unwrap_or(0);
    if confirmed < progress {
        return Err(ProgressError::List(ListError::Invalid(
            "MyAnimeList did not confirm the progress".into(),
        )));
    }
    Ok(confirmed)
}

// ---- Targets ----

/// The ids a stored list item carries (TheTVDB, MyAnimeList, AniList).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemIds {
    pub tvdb: Option<i64>,
    pub mal: Option<i64>,
    pub anilist: Option<i64>,
}

/// The library series progress is computed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeriesRef<'a> {
    /// The owning catalogue (`aniList`, `myAnimeList`, `tvdb`, …).
    pub provider: &'a str,
    pub metadata_id: &'a str,
    /// Whether an alternative episode ordering is in use (progress is never written then).
    pub ordered: bool,
}

/// The catalogue whose id identifies the remote title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    AniList,
    MyAnimeList,
    Tvdb,
}

impl IdKind {
    fn simkl_key(self) -> &'static str {
        match self {
            Self::AniList => "anilist",
            Self::MyAnimeList => "mal",
            Self::Tvdb => "tvdb",
        }
    }
}

/// `IImportListProgressWriter.TryGetTarget`: the remote title of a series on a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub kind: IdKind,
    pub remote_id: i64,
}

/// The list implementations that can receive progress.
pub const WRITERS: &[&str] = &[
    "AniListImport",
    "MyAnimeListImport",
    "TraktUserImport",
    "TraktListImport",
    "SimklUserImport",
];

/// Whether progress may be written to the list: a supported implementation with its own
/// "sync watched progress" setting on.
pub fn enabled(implementation: &str, s: &Map<String, Value>) -> bool {
    WRITERS.contains(&implementation) && crate::flag(s, "syncWatchedProgress", false)
}

/// The target of `series` on a list whose stored item is `item`, if the implementation can
/// write progress for it.
pub fn target(implementation: &str, series: SeriesRef<'_>, item: ItemIds) -> Option<Target> {
    if series.ordered {
        return None;
    }
    let own: Option<i64> = series.metadata_id.trim().parse().ok().filter(|n| *n > 0);
    let owned = |kind: IdKind, provider: &str, id: Option<i64>| {
        (series.provider.eq_ignore_ascii_case(provider) && own.is_some() && id == own).then(|| {
            Target {
                kind,
                remote_id: own.unwrap_or_default(),
            }
        })
    };
    match implementation {
        "AniListImport" => owned(IdKind::AniList, "aniList", item.anilist),
        "MyAnimeListImport" => owned(IdKind::MyAnimeList, "myAnimeList", item.mal),
        "TraktUserImport" | "TraktListImport" => owned(IdKind::Tvdb, "tvdb", item.tvdb),
        "SimklUserImport" => owned(IdKind::AniList, "aniList", item.anilist)
            .or_else(|| owned(IdKind::MyAnimeList, "myAnimeList", item.mal))
            .or_else(|| owned(IdKind::Tvdb, "tvdb", item.tvdb)),
        _ => None,
    }
}

/// One episode of the watched prefix, in order (`progress` N covers the first N).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEpisode {
    pub season: i64,
    pub episode: i64,
    /// The catalogue's episode id (TheTVDB's for TheTVDB-owned series).
    pub provider_id: Option<i64>,
    pub watched_at: Option<String>,
}

/// `Advance`: writes progress from `acknowledged` up to `pending`; returns the progress the
/// service confirms (possibly less than `pending` for Trakt and Simkl, which take batches).
pub async fn advance(
    http: &reqwest::Client,
    implementation: &str,
    s: &Map<String, Value>,
    target: Target,
    acknowledged: i64,
    pending: i64,
    episodes: &[ProgressEpisode],
) -> Result<i64> {
    match implementation {
        "AniListImport" => anilist(http, s, target.remote_id, pending).await,
        "MyAnimeListImport" => myanimelist(http, s, target.remote_id, pending).await,
        "TraktUserImport" | "TraktListImport" => {
            trakt(http, s, target.remote_id, acknowledged, pending, episodes).await
        }
        "SimklUserImport" => simkl(http, s, target, acknowledged, pending, episodes).await,
        other => Err(unavailable(format!("{other} cannot receive progress"))),
    }
}

async fn json_of(resp: reqwest::Response, service: &str) -> Result<Value> {
    check_status(&resp)?;
    resp.json::<Value>().await.map_err(|_| {
        ProgressError::List(ListError::Invalid(format!("{service} did not answer JSON")))
    })
}

/// `TraktProgressWriter`: marks the next (at most 20) episodes watched, one history entry each,
/// skipping episodes the account already has in its history. Trakt does not deduplicate, so a
/// write is never repeated blindly.
pub async fn trakt(
    http: &reqwest::Client,
    s: &Map<String, Value>,
    show_tvdb: i64,
    acknowledged: i64,
    pending: i64,
    episodes: &[ProgressEpisode],
) -> Result<i64> {
    let client_id =
        text(s, "clientId").ok_or_else(|| unavailable("Trakt progress needs a Client ID"))?;
    let token =
        text(s, "accessToken").ok_or_else(|| unavailable("Trakt progress needs a token"))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.trakt.tv".into());
    let base = base.trim_end_matches('/').to_string();
    let req = |m: reqwest::Method, path: String| {
        http.request(m, format!("{base}{path}"))
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &client_id)
            .bearer_auth(&token)
    };
    let me = json_of(
        req(reqwest::Method::GET, "/users/settings".into())
            .send()
            .await
            .map_err(connection_error)?,
        "Trakt",
    )
    .await?;
    let slug = me["user"]["ids"]["slug"].as_str().unwrap_or_default();
    let name = me["user"]["username"].as_str().unwrap_or_default();
    if slug.is_empty() {
        return Err(unavailable("Trakt did not name the token's account"));
    }
    if let Some(auth) = text(s, "authUser")
        && !auth.eq_ignore_ascii_case(slug)
    {
        return Err(unavailable(
            "the Trakt token belongs to another account than the list",
        ));
    }
    if let Some(user) = text(s, "username")
        && !user.eq_ignore_ascii_case(slug)
        && !user.eq_ignore_ascii_case(name)
    {
        return Err(unavailable(
            "the Trakt token belongs to another account than the list",
        ));
    }
    let shows = json_of(
        req(
            reqwest::Method::GET,
            format!("/search/tvdb/{show_tvdb}?type=show"),
        )
        .send()
        .await
        .map_err(connection_error)?,
        "Trakt",
    )
    .await?;
    let [show] = shows.as_array().map(Vec::as_slice).unwrap_or_default() else {
        return Err(unavailable(
            "Trakt does not know the show by its TheTVDB id",
        ));
    };
    let show_trakt = show["show"]["ids"]["trakt"].as_i64();
    let mut confirmed = acknowledged;
    let first = acknowledged.max(0) as usize;
    let last = (pending.max(0) as usize)
        .min(episodes.len())
        .min(first + 20);
    for e in episodes.get(first..last).unwrap_or_default() {
        let Some(ep_tvdb) = e.provider_id else {
            break;
        };
        let found = json_of(
            req(
                reqwest::Method::GET,
                format!("/search/tvdb/{ep_tvdb}?type=episode"),
            )
            .send()
            .await
            .map_err(connection_error)?,
            "Trakt",
        )
        .await?;
        let [hit] = found.as_array().map(Vec::as_slice).unwrap_or_default() else {
            break;
        };
        let (Some(ep_trakt), true) = (
            hit["episode"]["ids"]["trakt"].as_i64(),
            hit["show"]["ids"]["trakt"].as_i64() == show_trakt,
        ) else {
            break;
        };
        let history = json_of(
            req(
                reqwest::Method::GET,
                format!("/sync/history/episodes/{ep_trakt}?limit=1"),
            )
            .send()
            .await
            .map_err(connection_error)?,
            "Trakt",
        )
        .await?;
        if !history.as_array().is_some_and(|h| !h.is_empty()) {
            let at = e.watched_at.clone().unwrap_or_else(chrono_now);
            let added = json_of(
                req(reqwest::Method::POST, "/sync/history".into())
                    .json(&json!({"episodes": [{"ids": {"trakt": ep_trakt}, "watched_at": at}]}))
                    .send()
                    .await
                    .map_err(connection_error)?,
                "Trakt",
            )
            .await?;
            let not_found_empty = added["not_found"]
                .as_object()
                .is_none_or(|m| m.values().all(|v| v.as_array().is_none_or(Vec::is_empty)));
            if added["added"]["episodes"].as_i64() != Some(1) || !not_found_empty {
                break;
            }
        }
        confirmed += 1;
    }
    if confirmed == acknowledged && acknowledged < pending {
        return Err(ProgressError::List(ListError::Invalid(
            "Trakt did not confirm the next episode".into(),
        )));
    }
    Ok(confirmed)
}

/// `SimklProgressWriter`: adds the next (at most 100) episodes to the account's history.
pub async fn simkl(
    http: &reqwest::Client,
    s: &Map<String, Value>,
    target: Target,
    acknowledged: i64,
    pending: i64,
    episodes: &[ProgressEpisode],
) -> Result<i64> {
    let client_id = text(s, "clientIdV2")
        .or_else(|| text(s, "clientId"))
        .ok_or_else(|| unavailable("Simkl progress needs a Client ID"))?;
    let token =
        text(s, "accessToken").ok_or_else(|| unavailable("Simkl progress needs a token"))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.simkl.com".into());
    let base = base.trim_end_matches('/').to_string();
    let req = |m: reqwest::Method, path: &str| {
        http.request(m, format!("{base}{path}"))
            .header("simkl-api-key", &client_id)
            .bearer_auth(&token)
    };
    let me = json_of(
        req(reqwest::Method::GET, "/users/settings")
            .send()
            .await
            .map_err(connection_error)?,
        "Simkl",
    )
    .await?;
    let account = me["account"]["id"]
        .as_i64()
        .map(|n| n.to_string())
        .or_else(|| me["account"]["id"].as_str().map(str::to_string));
    let Some(account) = account else {
        return Err(unavailable("Simkl did not name the token's account"));
    };
    if let Some(auth) = text(s, "authUser")
        && auth != account
    {
        return Err(unavailable(
            "the Simkl token belongs to another account than the list",
        ));
    }
    let key = target.kind.simkl_key();
    let watched = json_of(
        req(reqwest::Method::POST, "/sync/watched")
            .json(&json!([{"ids": {key: target.remote_id}, "type": "show"}]))
            .send()
            .await
            .map_err(connection_error)?,
        "Simkl",
    )
    .await?;
    let [entry] = watched.as_array().map(Vec::as_slice).unwrap_or_default() else {
        return Err(unavailable("Simkl does not know the title"));
    };
    let simkl_id = entry["simkl"]
        .as_i64()
        .or_else(|| entry["ids"]["simkl"].as_i64());
    let (Some(simkl_id), false) = (simkl_id, entry["list"].is_null()) else {
        return Err(unavailable("the title is not on the Simkl list"));
    };
    let first = acknowledged.max(0) as usize;
    let last = (pending.max(0) as usize)
        .min(episodes.len())
        .min(first + 100);
    let batch = episodes.get(first..last).unwrap_or_default();
    if batch.is_empty() {
        return Ok(acknowledged);
    }
    let mut seasons: Vec<(i64, Vec<Value>)> = Vec::new();
    for e in batch {
        let number = json!({"number": e.episode});
        match seasons.last_mut() {
            Some((s, eps)) if *s == e.season => eps.push(number),
            _ => seasons.push((e.season, vec![number])),
        }
    }
    let mut show = json!({
        "ids": {"simkl": simkl_id},
        "seasons": seasons.into_iter().map(|(n, eps)| json!({"number": n, "episodes": eps})).collect::<Vec<_>>(),
    });
    if target.kind == IdKind::Tvdb {
        show["use_tvdb_anime_seasons"] = json!(true);
    }
    let added = json_of(
        req(
            reqwest::Method::POST,
            "/sync/history?skip_auto_watching=yes",
        )
        .json(&json!({"shows": [show]}))
        .send()
        .await
        .map_err(connection_error)?,
        "Simkl",
    )
    .await?;
    let not_found_empty = added["not_found"]
        .as_object()
        .is_none_or(|m| m.values().all(|v| v.as_array().is_none_or(Vec::is_empty)));
    if added["added"]["episodes"].as_i64().is_none() || !not_found_empty {
        return Err(ProgressError::List(ListError::Invalid(
            "Simkl did not confirm the episodes".into(),
        )));
    }
    Ok(acknowledged + batch.len() as i64)
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_ownership() {
        let anilist = SeriesRef {
            provider: "aniList",
            metadata_id: "21",
            ordered: false,
        };
        let ids = ItemIds {
            anilist: Some(21),
            mal: Some(5),
            tvdb: None,
        };
        assert_eq!(
            target("AniListImport", anilist, ids),
            Some(Target {
                kind: IdKind::AniList,
                remote_id: 21
            })
        );
        assert_eq!(target("MyAnimeListImport", anilist, ids), None);
        assert_eq!(
            target(
                "AniListImport",
                SeriesRef {
                    ordered: true,
                    ..anilist
                },
                ids
            ),
            None
        );
        assert_eq!(
            target(
                "AniListImport",
                anilist,
                ItemIds {
                    anilist: Some(22),
                    ..ids
                }
            ),
            None
        );
        let tvdb = SeriesRef {
            provider: "tvdb",
            metadata_id: "81189",
            ordered: false,
        };
        let t = ItemIds {
            tvdb: Some(81189),
            ..Default::default()
        };
        assert_eq!(
            target("TraktUserImport", tvdb, t),
            Some(Target {
                kind: IdKind::Tvdb,
                remote_id: 81189
            })
        );
        assert_eq!(
            target("SimklUserImport", tvdb, t),
            Some(Target {
                kind: IdKind::Tvdb,
                remote_id: 81189
            })
        );
        assert_eq!(target("TmdbListImport", tvdb, t), None);
    }
}
