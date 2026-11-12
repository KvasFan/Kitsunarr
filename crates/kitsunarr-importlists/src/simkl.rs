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
    let simkl_id = |e: &Value| positive(&e["show"]["ids"]["simkl"]);
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
    // Tanukarr's custom lists: the watchlist rows narrowed to the included lists and without
    // the excluded ones, matched by Simkl id.
    let included = custom_list_members(http, s, &base, "includedCustomListIds").await?;
    let excluded = custom_list_members(http, s, &base, "excludedCustomListIds").await?;
    let keep = |e: &Value| {
        let id = simkl_id(e);
        included
            .as_ref()
            .is_none_or(|ids| id.is_some_and(|i| ids.contains(&i)))
            && excluded
                .as_ref()
                .is_none_or(|ids| id.is_none_or(|i| !ids.contains(&i)))
    };
    let mut out: Vec<ListItem> = body["shows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| keep(e))
        .map(map)
        .collect();
    for a in body["anime"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| keep(e))
    {
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

/// The Simkl ids of the custom lists in `key` (`None` when there are none): `lists/{id}`,
/// 500 a page, at most 20 pages (Tanukarr's `SimklCustomListReader`).
async fn custom_list_members(
    http: &reqwest::Client,
    s: &Map<String, Value>,
    base: &str,
    key: &str,
) -> Result<Option<std::collections::HashSet<i64>>, ListError> {
    let ids = crate::ints(s, key);
    if ids.is_empty() {
        return Ok(None);
    }
    let client_id = text(s, "clientIdV2")
        .or_else(|| text(s, "clientId"))
        .unwrap_or_default();
    let token = text(s, "accessToken").unwrap_or_default();
    let mut members = std::collections::HashSet::new();
    for id in ids.into_iter().filter(|i| *i > 0) {
        for page in 1..=20 {
            let resp = http
                .get(format!("{}/lists/{id}", base.trim_end_matches('/')))
                .query(&[("limit", "500"), ("page", &page.to_string())])
                .header("simkl-api-key", &client_id)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(connection_error)?;
            if resp.status().as_u16() == 404 {
                return Err(ListError::Settings(format!(
                    "Simkl has no custom list {id}"
                )));
            }
            check_status(&resp)?;
            let body: Value = resp.json().await.map_err(|_| {
                ListError::Invalid(
                    "Simkl did not answer a custom list; the previous list is kept".into(),
                )
            })?;
            let items = body["items"]
                .as_array()
                .or_else(|| body.as_array())
                .cloned()
                .unwrap_or_default();
            for item in &items {
                let member = positive(&item["simkl_id"])
                    .or_else(|| positive(&item["ids"]["simkl"]))
                    .or_else(|| positive(&item["show"]["ids"]["simkl"]))
                    .or_else(|| positive(&item["anime"]["ids"]["simkl"]));
                match member {
                    Some(m) => {
                        members.insert(m);
                    }
                    None => {
                        return Err(ListError::Invalid(
                            "a Simkl custom list member has no Simkl id; the previous list is kept"
                                .into(),
                        ));
                    }
                }
            }
            if items.len() < 500 {
                break;
            }
            if page == 20 {
                return Err(ListError::Invalid(
                    "a Simkl custom list has more than 10000 members".into(),
                ));
            }
        }
    }
    Ok(Some(members))
}
