//! AniList (`AniListImport`, Tanukarr): a user's anime list filtered by list status, media
//! release status and custom lists. Public lists are read without a token; a token (migrated
//! from Tanukarr or typed) reads private ones.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::{ListError, ListItem, check_status, connection_error, flag, strings, text};

const MAX_PAGES: i64 = 100;

const QUERY: &str = "query ($id: String!, $statusType: [MediaListStatus!]!, $page: Int!) {
  Page(page: $page, perPage: 50) {
    pageInfo { currentPage hasNextPage }
    mediaList(userName: $id, type: ANIME, status_in: $statusType) {
      status progress
      customListEntries: customLists(asArray: true)
      media { id idMal format status title { userPreferred romaji } startDate { year } }
    }
  }
}";

/// Tanukarr's defaults: watching, planning, releasing and finished.
fn list_statuses(s: &Map<String, Value>) -> Vec<&'static str> {
    [
        ("importCompleted", "COMPLETED", false),
        ("importCurrent", "CURRENT", true),
        ("importDropped", "DROPPED", false),
        ("importPaused", "PAUSED", false),
        ("importPlanning", "PLANNING", true),
        ("importRepeating", "REPEATING", false),
    ]
    .into_iter()
    .filter(|(k, _, d)| flag(s, k, *d))
    .map(|(_, v, _)| v)
    .collect()
}

fn media_status_selected(s: &Map<String, Value>, status: &str) -> bool {
    match status {
        "FINISHED" => flag(s, "importFinished", true),
        "RELEASING" => flag(s, "importReleasing", true),
        "NOT_YET_RELEASED" => flag(s, "importUnreleased", false),
        "CANCELLED" => flag(s, "importCancelled", false),
        "HIATUS" => flag(s, "importHiatus", false),
        _ => false,
    }
}

/// Membership by exact name, else by a single case-insensitive match (Tanukarr's rule; an
/// ambiguous excluded name is an error).
fn member(lists: &[(String, bool)], name: &str, exclude: bool) -> Result<bool, ListError> {
    if let Some((_, m)) = lists.iter().find(|(n, _)| n == name) {
        return Ok(*m);
    }
    let ci: Vec<_> = lists
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .collect();
    if exclude && ci.len() > 1 {
        return Err(ListError::Settings(format!(
            "the excluded AniList custom list {name} is ambiguous by letter case; use its exact name"
        )));
    }
    Ok(ci.len() == 1 && ci[0].1)
}

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let user = text(s, "username").ok_or(ListError::Settings(
        "The AniList username is required".into(),
    ))?;
    let statuses = list_statuses(s);
    if statuses.is_empty() {
        return Err(ListError::Settings(
            "Select at least one list status to import".into(),
        ));
    }
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://graphql.anilist.co".into());
    let token = text(s, "accessToken");
    let mut included = strings(s, "includedCustomLists");
    if let Some(c) = text(s, "customList") {
        included.push(c);
    }
    let excluded = strings(s, "excludedCustomLists");
    let invalid = |m: &str| ListError::Invalid(format!("AniList {m}; the previous list is kept"));

    let mut excluded_ids = HashSet::new();
    let mut out: Vec<ListItem> = Vec::new();
    for page in 1..=MAX_PAGES {
        let mut req = http
            .post(&base)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&json!({"query": QUERY, "variables": {"id": user, "statusType": statuses, "page": page}}));
        if let Some(t) = &token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.map_err(connection_error)?;
        if resp.status().as_u16() == 404 {
            return Err(ListError::Settings(format!(
                "the AniList user {user} does not exist or the list is private"
            )));
        }
        check_status(&resp)?;
        let doc: Value = resp
            .json()
            .await
            .map_err(|_| invalid("did not answer JSON"))?;
        if let Some(errors) = doc["errors"].as_array().filter(|e| !e.is_empty()) {
            let msg: Vec<&str> = errors
                .iter()
                .filter_map(|e| e["message"].as_str())
                .collect();
            return Err(ListError::Invalid(format!(
                "AniList returned errors: {}",
                msg.join("; ")
            )));
        }
        let p = &doc["data"]["Page"];
        let (Some(entries), Some(has_next)) = (
            p["mediaList"].as_array(),
            p["pageInfo"]["hasNextPage"].as_bool(),
        ) else {
            return Err(invalid("returned an incomplete list response"));
        };
        for e in entries {
            let media = &e["media"];
            let Some(id) = media["id"].as_i64().filter(|i| *i > 0) else {
                return Err(invalid("returned a list entry without a valid media id"));
            };
            let lists: Option<Vec<(String, bool)>> = e["customListEntries"].as_array().map(|a| {
                a.iter()
                    .filter_map(|l| {
                        Some((l["name"].as_str()?.to_string(), l["enabled"].as_bool()?))
                    })
                    .collect()
            });
            if !excluded.is_empty() {
                let l = lists
                    .as_deref()
                    .ok_or_else(|| invalid("returned an entry without custom list membership"))?;
                let mut hit = false;
                for name in &excluded {
                    hit |= member(l, name, true)?;
                }
                if hit {
                    excluded_ids.insert(id);
                    continue;
                }
            }
            if excluded_ids.contains(&id)
                || !statuses.contains(&e["status"].as_str().unwrap_or_default())
                || !media_status_selected(s, media["status"].as_str().unwrap_or_default())
            {
                continue;
            }
            if !included.is_empty() {
                let Some(l) = lists.as_deref() else { continue };
                let mut on = false;
                for name in &included {
                    on |= member(l, name, false)?;
                }
                if !on {
                    continue;
                }
            }
            let title = media["title"]["userPreferred"]
                .as_str()
                .filter(|t| !t.trim().is_empty())
                .or(media["title"]["romaji"].as_str())
                .unwrap_or_default();
            out.push(ListItem {
                title: title.to_string(),
                year: media["startDate"]["year"].as_i64(),
                anilist_id: Some(id),
                mal_id: media["idMal"].as_i64().filter(|i| *i > 0),
                ..Default::default()
            });
        }
        if !has_next {
            out.retain(|i| !i.anilist_id.is_some_and(|a| excluded_ids.contains(&a)));
            return Ok(out);
        }
    }
    Err(invalid("lists more than 5000 entries"))
}
