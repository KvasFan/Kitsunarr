//! Account-list pickers (Tanukarr's `getCustomLists` select options): the lists an account
//! holds, for the include and exclude fields of AniList, Trakt and Simkl lists.

use serde_json::{Map, Value, json};

use crate::{ListError, check_status, connection_error, text};

/// One list to choose: its value for the setting and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub name: String,
}

/// The account's lists for an implementation's include/exclude fields.
pub async fn lists(
    http: &reqwest::Client,
    implementation: &str,
    s: &Map<String, Value>,
) -> Result<Vec<Choice>, ListError> {
    match implementation {
        "AniListImport" => anilist(http, s).await,
        "TraktListImport" | "TraktUserImport" => trakt(http, s).await,
        "SimklUserImport" => simkl(http, s).await,
        other => Err(ListError::Settings(format!(
            "{other} has no account lists to choose"
        ))),
    }
}

/// AniList's custom anime lists of the user (`mediaListOptions.animeList.customLists`).
async fn anilist(http: &reqwest::Client, s: &Map<String, Value>) -> Result<Vec<Choice>, ListError> {
    let user = text(s, "username").ok_or(ListError::Settings(
        "Enter the AniList username first".into(),
    ))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://graphql.anilist.co".into());
    let mut req = http.post(&base).json(&json!({
        "query": "query ($name: String) { User(name: $name) { mediaListOptions { animeList { customLists } } } }",
        "variables": {"name": user},
    }));
    if let Some(t) = text(s, "accessToken") {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(connection_error)?;
    check_status(&resp)?;
    let v: Value = resp
        .json()
        .await
        .map_err(|_| ListError::Invalid("AniList did not answer JSON".into()))?;
    Ok(
        v["data"]["User"]["mediaListOptions"]["animeList"]["customLists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|n| Choice {
                value: n.to_string(),
                name: n.to_string(),
            })
            .collect(),
    )
}

/// The Trakt user's personal lists (`users/{user}/lists`), by slug.
async fn trakt(http: &reqwest::Client, s: &Map<String, Value>) -> Result<Vec<Choice>, ListError> {
    let client_id = text(s, "clientId").ok_or(ListError::Settings(
        "Enter the Trakt Client ID first".into(),
    ))?;
    let user = text(s, "username")
        .or_else(|| text(s, "authUser"))
        .ok_or(ListError::Settings("Enter the Trakt username first".into()))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.trakt.tv".into());
    let mut req = http
        .get(format!("{}/users/{user}/lists", base.trim_end_matches('/')))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id);
    if let Some(t) = text(s, "accessToken") {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(connection_error)?;
    check_status(&resp)?;
    let v: Value = resp
        .json()
        .await
        .map_err(|_| ListError::Invalid("Trakt did not answer JSON".into()))?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| {
            let slug = l["ids"]["slug"].as_str()?;
            Some(Choice {
                value: slug.to_string(),
                name: l["name"].as_str().unwrap_or(slug).to_string(),
            })
        })
        .collect())
}

/// The Simkl account's custom lists (`lists/user/{account}`), by id.
async fn simkl(http: &reqwest::Client, s: &Map<String, Value>) -> Result<Vec<Choice>, ListError> {
    let client_id = text(s, "clientIdV2")
        .or_else(|| text(s, "clientId"))
        .ok_or(ListError::Settings(
            "Enter the Simkl Client ID first".into(),
        ))?;
    let token = text(s, "accessToken").ok_or(ListError::Settings(
        "Enter the Simkl access token first".into(),
    ))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.simkl.com".into());
    let base = base.trim_end_matches('/').to_string();
    let get = |path: String| {
        http.get(format!("{base}{path}"))
            .header("simkl-api-key", &client_id)
            .bearer_auth(&token)
    };
    let me = get("/users/settings".into())
        .send()
        .await
        .map_err(connection_error)?;
    check_status(&me)?;
    let me: Value = me
        .json()
        .await
        .map_err(|_| ListError::Invalid("Simkl did not answer JSON".into()))?;
    let account = me["account"]["id"]
        .as_i64()
        .ok_or(ListError::Invalid("Simkl did not name the account".into()))?;
    let resp = get(format!("/lists/user/{account}?limit=500"))
        .send()
        .await
        .map_err(connection_error)?;
    check_status(&resp)?;
    let v: Value = resp
        .json()
        .await
        .map_err(|_| ListError::Invalid("Simkl did not answer JSON".into()))?;
    let anime = s.get("showType").and_then(Value::as_i64) == Some(1);
    Ok(v["lists"]
        .as_array()
        .or_else(|| v.as_array())
        .into_iter()
        .flatten()
        .filter(|l| {
            l["media_type"]
                .as_str()
                .is_none_or(|t| (t == "anime") == anime)
        })
        .filter_map(|l| {
            let id = l["id"].as_i64()?;
            Some(Choice {
                value: id.to_string(),
                name: l["name"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect())
}
