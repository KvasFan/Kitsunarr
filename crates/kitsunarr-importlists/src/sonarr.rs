//! Another Sonarr (`SonarrImport`): its series, filtered by quality profile, language profile,
//! tag and root folder; optionally with its season monitoring.

use serde_json::{Map, Value};

use crate::{
    ListError, ListItem, check_status, connection_error, flag, ints, positive, strings, text,
};

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let base =
        text(s, "baseUrl").ok_or(ListError::Settings("The Sonarr URL is required".into()))?;
    let key = text(s, "apiKey").ok_or(ListError::Settings("The API key is required".into()))?;
    let resp = http
        .get(format!("{}/api/v3/series", base.trim_end_matches('/')))
        .header("X-Api-Key", key)
        .send()
        .await
        .map_err(connection_error)?;
    check_status(&resp)?;
    let series: Vec<Value> = resp
        .json()
        .await
        .map_err(|_| ListError::Invalid("Sonarr did not answer a series list".into()))?;
    let profiles = ints(s, "profileIds");
    let languages = ints(s, "languageProfileIds");
    let tags = ints(s, "tagIds");
    let roots: Vec<String> = strings(s, "rootFolderPaths")
        .into_iter()
        .map(|r| r.to_lowercase())
        .collect();
    let seasons = flag(s, "syncSeasonMonitoring", false);
    Ok(series
        .iter()
        .filter(|x| {
            profiles.is_empty()
                || x["qualityProfileId"]
                    .as_i64()
                    .is_some_and(|p| profiles.contains(&p))
        })
        .filter(|x| {
            languages.is_empty()
                || x["languageProfileId"]
                    .as_i64()
                    .is_some_and(|p| languages.contains(&p))
        })
        .filter(|x| {
            tags.is_empty()
                || x["tags"].as_array().is_some_and(|t| {
                    t.iter()
                        .filter_map(Value::as_i64)
                        .any(|t| tags.contains(&t))
                })
        })
        .filter(|x| {
            roots.is_empty() || {
                let r = x["rootFolderPath"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase();
                roots.iter().any(|root| r.contains(root.as_str()))
            }
        })
        .map(|x| ListItem {
            title: x["title"].as_str().unwrap_or_default().to_string(),
            tvdb_id: positive(&x["tvdbId"]),
            seasons: if seasons {
                x["seasons"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| {
                                Some((
                                    s["seasonNumber"].as_i64()?,
                                    s["monitored"].as_bool().unwrap_or(false),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            ..Default::default()
        })
        .collect())
}
