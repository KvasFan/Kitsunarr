//! MyAnimeList (`MyAnimeListImport`, Tanukarr): a user's anime list through the MyAnimeList API
//! v2, filtered by list status, release status, score and tags. Public lists need the user's own
//! registered Client ID; a token (migrated from Tanukarr) reads `@me` and private lists and is
//! renewed with the refresh token before it expires.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::{ListError, ListItem, check_status, connection_error, flag, ints, strings, text};

const MAX_PAGES: usize = 100;
const PAGE: usize = 1000;
pub const TOKEN_URL: &str = "https://myanimelist.net/v1/oauth2/token";

/// `MyAnimeListStatus` (All = 0) → the API's list status.
fn status_name(code: i64) -> Option<&'static str> {
    match code {
        1 => Some("watching"),
        2 => Some("completed"),
        3 => Some("on_hold"),
        4 => Some("dropped"),
        5 => Some("plan_to_watch"),
        _ => None,
    }
}

const STATUSES: [&str; 5] = [
    "watching",
    "completed",
    "on_hold",
    "dropped",
    "plan_to_watch",
];

/// The settings with a renewed token when the stored one expires within five minutes; `None`
/// when no renewal is needed.
pub async fn renew(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>, ListError> {
    if text(s, "accessToken").is_none() {
        return Ok(None);
    }
    let expires = text(s, "expires")
        .and_then(|e| chrono_like_parse(&e))
        .unwrap_or(0);
    if expires > now_secs() + 300 {
        return Ok(None);
    }
    let (Some(client_id), Some(refresh)) = (text(s, "clientId"), text(s, "refreshToken")) else {
        return Err(ListError::Authentication(
            "the MyAnimeList token needs renewal: enter your registered Client ID (and secret) and a refresh token".into(),
        ));
    };
    let mut form = vec![
        ("client_id", client_id),
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh),
    ];
    if let Some(secret) = text(s, "clientSecret") {
        form.push(("client_secret", secret));
    }
    let url = text(s, "tokenUrl").unwrap_or_else(|| TOKEN_URL.into());
    let resp = http
        .post(url)
        .form(&form)
        .send()
        .await
        .map_err(connection_error)?;
    if !resp.status().is_success() {
        return Err(ListError::Authentication(format!(
            "MyAnimeList refused to renew the token (HTTP {}); sign in again",
            resp.status().as_u16()
        )));
    }
    let t: Value = resp
        .json()
        .await
        .map_err(|_| ListError::Invalid("MyAnimeList sent an unreadable token".into()))?;
    let (Some(access), Some(expires_in)) = (t["access_token"].as_str(), t["expires_in"].as_i64())
    else {
        return Err(ListError::Invalid(
            "MyAnimeList sent an incomplete token".into(),
        ));
    };
    let mut next = s.clone();
    next.insert("accessToken".into(), json!(access));
    if let Some(r) = t["refresh_token"].as_str() {
        next.insert("refreshToken".into(), json!(r));
    }
    next.insert(
        "expires".into(),
        json!(format_secs(now_secs() + expires_in)),
    );
    Ok(Some(next))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Seconds since the epoch of `YYYY-MM-DDTHH:MM:SS…` (UTC; an offset or fraction is ignored).
fn chrono_like_parse(s: &str) -> Option<i64> {
    let b = s.get(..19)?;
    let n = |r: std::ops::Range<usize>| b.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    // Days from the civil date (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

fn format_secs(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let user = text(s, "username").unwrap_or_else(|| "@me".into());
    let token = text(s, "accessToken");
    let client_id = text(s, "clientId");
    if token.is_none() && client_id.is_none() {
        return Err(ListError::Settings(
            "Enter your registered MyAnimeList Client ID (https://myanimelist.net/apiconfig)"
                .into(),
        ));
    }
    if token.is_none() && user == "@me" {
        return Err(ListError::Settings(
            "Enter the MyAnimeList username of a public list".into(),
        ));
    }
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.myanimelist.net/v2".into());
    let mut codes = ints(s, "listStatuses");
    if codes.is_empty() {
        codes.push(s.get("listStatus").and_then(Value::as_i64).unwrap_or(0));
    }
    let all = codes.contains(&0);
    let selected: HashSet<&str> = codes.iter().filter_map(|c| status_name(*c)).collect();
    let included: HashSet<String> = strings(s, "includedTags").into_iter().collect();
    let excluded: HashSet<String> = strings(s, "excludedTags").into_iter().collect();
    let min_score = s.get("minimumScore").and_then(Value::as_i64).unwrap_or(0);
    let (finished, airing, unreleased) = (
        flag(s, "importFinished", true),
        flag(s, "importAiring", true),
        flag(s, "importUnreleased", true),
    );
    let invalid =
        |m: &str| ListError::Invalid(format!("MyAnimeList {m}; the previous list is kept"));
    let user_segment = if user == "@me" {
        user.clone()
    } else {
        url_escape(&user)
    };

    let mut out = Vec::new();
    let mut excluded_ids = HashSet::new();
    let mut seen = HashSet::new();
    let mut offset = 0usize;
    for _ in 0..MAX_PAGES {
        let mut req = http
            .get(format!(
                "{}/users/{user_segment}/animelist",
                base.trim_end_matches('/')
            ))
            .query(&[
                (
                    "fields",
                    "status,list_status{status,score,tags,is_rewatching}",
                ),
                ("limit", &PAGE.to_string()),
                ("offset", &offset.to_string()),
            ]);
        req = match (&token, &client_id) {
            (Some(t), _) => req.bearer_auth(t),
            (None, Some(c)) => req.header("X-MAL-CLIENT-ID", c),
            _ => req,
        };
        let resp = req.send().await.map_err(connection_error)?;
        if resp.status().as_u16() == 404 {
            return Err(ListError::Settings(format!(
                "the MyAnimeList user {user} does not exist or the list is private"
            )));
        }
        check_status(&resp)?;
        let doc: Value = resp
            .json()
            .await
            .map_err(|_| invalid("did not answer JSON"))?;
        let (Some(data), Some(paging)) = (doc["data"].as_array(), doc["paging"].as_object()) else {
            return Err(invalid("returned an incomplete page"));
        };
        if data.len() > PAGE {
            return Err(invalid("returned an oversized page"));
        }
        for e in data {
            let node = &e["node"];
            let ls = &e["list_status"];
            let (Some(id), Some(title)) = (
                node["id"].as_i64().filter(|i| *i > 0),
                node["title"].as_str().filter(|t| !t.trim().is_empty()),
            ) else {
                return Err(invalid("returned an entry without an id or title"));
            };
            let score = ls["score"]
                .as_i64()
                .filter(|s| (0..=10).contains(s))
                .ok_or_else(|| invalid("returned an invalid score"))?;
            let status = ls["status"]
                .as_str()
                .filter(|s| STATUSES.contains(s))
                .ok_or_else(|| invalid("returned an unknown list status"))?;
            let tags: Option<Vec<String>> = ls["tags"].as_array().map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            });
            if (!included.is_empty() || !excluded.is_empty()) && tags.is_none() {
                return Err(invalid(
                    "did not return the tags needed for the tag filters",
                ));
            }
            let tags = tags.unwrap_or_default();
            if tags.iter().any(|t| excluded.contains(t)) {
                excluded_ids.insert(id);
                continue;
            }
            let account_status = if ls["is_rewatching"].as_bool().unwrap_or(false) {
                "watching"
            } else {
                status
            };
            let release_ok = match node["status"].as_str() {
                Some("finished_airing") => finished,
                Some("currently_airing") => airing,
                Some("not_yet_aired") => unreleased,
                _ if finished && airing && unreleased => true,
                _ => {
                    return Err(invalid(
                        "did not return the release status needed for the filters",
                    ));
                }
            };
            if !release_ok
                || !(all || selected.contains(account_status))
                || score < min_score
                || (!included.is_empty() && !tags.iter().any(|t| included.contains(t)))
                || excluded_ids.contains(&id)
                || !seen.insert(id)
            {
                continue;
            }
            out.push(ListItem {
                title: title.to_string(),
                mal_id: Some(id),
                ..Default::default()
            });
        }
        let Some(next) = paging
            .get("next")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
        else {
            out.retain(|i: &ListItem| !i.mal_id.is_some_and(|m| excluded_ids.contains(&m)));
            return Ok(out);
        };
        let following = url::Url::parse(next).ok().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "offset")
                .and_then(|(_, v)| v.parse::<usize>().ok())
        });
        if data.is_empty() || following != Some(offset + data.len()) {
            return Err(invalid("returned a repeated or skipped page"));
        }
        offset += data.len();
    }
    Err(invalid("exceeded the 100-page limit"))
}

fn url_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_round_trip() {
        let t = super::chrono_like_parse("2026-11-12T10:20:30Z").unwrap();
        assert_eq!(super::format_secs(t), "2026-11-12T10:20:30Z");
        assert_eq!(super::chrono_like_parse("1970-01-01T00:00:00"), Some(0));
        assert_eq!(super::format_secs(951_782_400), "2000-02-29T00:00:00Z");
    }
}
