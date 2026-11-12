//! Simkl (`SimklUserImport`): a user's shows or anime in one list status, through
//! `/sync/all-items`. Simkl needs a registered application: its Client ID (`clientIdV2`, Tanukarr's
//! AUTH V2 field) and the user's access token.

use serde_json::{Map, Value};

use crate::{ListError, ListItem, check_status, connection_error, imdb, positive, text};

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let client_id = text(s, "clientIdV2")
        .or_else(|| text(s, "clientId"))
        .ok_or(ListError::Settings(
            "Enter the Client ID of your Simkl application (https://simkl.com/settings/developer/)"
                .into(),
        ))?;
    let token = text(s, "accessToken").ok_or(ListError::Settings(
        "Enter the Simkl access token of your application".into(),
    ))?;
    let kind = match s.get("showType").and_then(Value::as_i64) {
        Some(1) => "anime",
        _ => "shows",
    };
    let status = match s.get("listType").and_then(Value::as_i64) {
        Some(1) => "plantowatch",
        Some(2) => "hold",
        Some(3) => "completed",
        Some(4) => "dropped",
        _ => "watching",
    };
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.simkl.com".into());
    let resp = http
        .get(format!(
            "{}/sync/all-items/{kind}/{status}",
            base.trim_end_matches('/')
        ))
        .header("simkl-api-key", client_id)
        .bearer_auth(token)
        .send()
        .await
        .map_err(connection_error)?;
    check_status(&resp)?;
    let body: Value = resp.json().await.map_err(|_| {
        ListError::Invalid("Simkl did not answer JSON; the previous list is kept".into())
    })?;
    if body.is_null() {
        // Simkl answers `null` for an empty list.
        return Ok(Vec::new());
    }
    let map = |e: &Value| {
        let show = &e["show"];
        let ids = &show["ids"];
        ListItem {
            title: show["title"].as_str().unwrap_or_default().to_string(),
            year: show["year"].as_i64(),
            tvdb_id: positive(&ids["tvdb"]),
            tmdb_id: positive(&ids["tmdb"]),
            imdb_id: imdb(ids["imdb"].as_str()),
            mal_id: positive(&ids["mal"]),
            anilist_id: positive(&ids["anilist"]),
            ..Default::default()
        }
    };
    let mut out: Vec<ListItem> = body["shows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(map)
        .collect();
    for a in body["anime"].as_array().into_iter().flatten() {
        // Sonarr skips films and music videos.
        if !matches!(
            a["anime_type"].as_str(),
            Some("tv" | "ona" | "ova" | "special")
        ) {
            continue;
        }
        let item = map(a);
        if item.tvdb_id.is_some()
            || item.tmdb_id.is_some()
            || item.imdb_id.is_some()
            || item.mal_id.is_some()
        {
            out.push(item);
        }
    }
    Ok(out)
}
