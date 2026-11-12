//! TMDB lists (`TmdbListImport`, Tanukarr): the TV shows of the included TMDB lists minus those
//! of the excluded ones, read through the v4 API with the user's API Read Access Token.

use serde_json::{Map, Value};

use crate::{ListError, ListItem, check_status, compose, connection_error, strings, text};

const MAX_PAGES: i64 = 1000;

pub async fn fetch(
    http: &reqwest::Client,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let token = text(s, "authToken").ok_or(ListError::Settings(
        "Enter your TMDB API Read Access Token".into(),
    ))?;
    let mut included = strings(s, "includedListIds");
    for legacy in ["listId", "accountListId"] {
        if let Some(v) = s.get(legacy).and_then(|v| {
            v.as_i64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(str::to_string))
        }) && !v.is_empty()
            && !included.contains(&v)
        {
            included.push(v);
        }
    }
    if included.is_empty() {
        return Err(ListError::Settings(
            "Add at least one included TMDB list id".into(),
        ));
    }
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.themoviedb.org".into());
    let mut inc = Vec::new();
    for id in &included {
        inc.push(read_list(http, &base, &token, id).await?);
    }
    let mut exc = Vec::new();
    for id in strings(s, "excludedListIds") {
        exc.push(read_list(http, &base, &token, &id).await?);
    }
    compose(inc, exc)
}

async fn read_list(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    id: &str,
) -> Result<Vec<ListItem>, ListError> {
    if id.parse::<u64>().map_or(true, |n| n == 0) {
        return Err(ListError::Settings(format!(
            "{id} is not a positive numeric TMDB list id"
        )));
    }
    let invalid = |m: &str| ListError::Invalid(format!("TMDB {m}; the previous list is kept"));
    let (mut out, mut raw) = (Vec::new(), 0i64);
    let (mut total_pages, mut expected): (Option<i64>, Option<i64>) = (None, None);
    for page in 1..=MAX_PAGES {
        let resp = http
            .get(format!("{}/4/list/{id}", base.trim_end_matches('/')))
            .bearer_auth(token)
            .query(&[("page", page)])
            .send()
            .await
            .map_err(connection_error)?;
        if resp.status().as_u16() == 404 {
            return Err(ListError::Settings(format!(
                "the TMDB list {id} does not exist or is private"
            )));
        }
        check_status(&resp)?;
        let body: Value = resp
            .json()
            .await
            .map_err(|_| invalid("did not answer JSON"))?;
        let (Some(items), Some(p), Some(pages)) = (
            body["results"].as_array(),
            body["page"].as_i64(),
            body["total_pages"].as_i64(),
        ) else {
            return Err(invalid("did not return complete list pagination"));
        };
        if p != page
            || !(0..=MAX_PAGES).contains(&pages)
            || total_pages.is_some_and(|t| t != pages)
            || (pages == 0 && !items.is_empty())
        {
            return Err(invalid("did not return complete list pagination"));
        }
        if let Some(t) = body.get("total_results").filter(|v| !v.is_null()) {
            let t = t
                .as_i64()
                .filter(|t| *t >= 0)
                .ok_or_else(|| invalid("returned inconsistent item counts"))?;
            if expected.is_some_and(|e| e != t) {
                return Err(invalid("returned inconsistent item counts"));
            }
            expected = Some(t);
        }
        if page < pages && items.is_empty() {
            return Err(invalid("omitted a list page"));
        }
        total_pages = Some(pages);
        raw += items.len() as i64;
        for i in items {
            let kind = i["media_type"].as_str().unwrap_or("tv");
            if kind == "tv"
                && let Some(tmdb) = i["id"].as_i64().filter(|n| *n > 0)
            {
                out.push(ListItem {
                    title: i["name"].as_str().unwrap_or_default().to_string(),
                    tmdb_id: Some(tmdb),
                    year: i["first_air_date"]
                        .as_str()
                        .and_then(|d| d.get(..4)?.parse().ok()),
                    ..Default::default()
                });
            }
        }
        if page >= pages {
            if expected.is_some_and(|e| e != raw) {
                return Err(invalid("returned an incomplete list"));
            }
            return Ok(out);
        }
    }
    Err(invalid("pagination could not be completed"))
}
/// `TmdbLanguage` hints in enum order (index 0 is "any").
pub(crate) const TMDB_LANGUAGES: &[&str] = &[
    "any", "ab", "aa", "af", "ak", "sq", "am", "ar", "an", "hy", "as", "av", "ae", "ay", "az",
    "bm", "ba", "eu", "be", "bn", "bi", "bs", "br", "bg", "my", "cn", "ca", "ch", "ce", "ny", "cv",
    "kw", "co", "cr", "hr", "cs", "da", "dv", "nl", "dz", "en", "eo", "et", "ee", "fo", "fj", "fi",
    "fr", "fy", "ff", "gd", "gl", "lg", "ka", "de", "el", "gn", "gu", "ht", "ha", "he", "hz", "hi",
    "ho", "hu", "is", "io", "ig", "id", "ia", "ie", "iu", "ik", "ga", "it", "ja", "jv", "kl", "kn",
    "kr", "ks", "kk", "km", "ki", "rw", "ky", "kv", "kg", "ko", "kj", "ku", "lo", "la", "lv", "lb",
    "li", "ln", "lt", "lu", "mk", "mg", "ms", "ml", "mt", "zh", "gv", "mi", "mr", "mh", "mo", "mn",
    "na", "nv", "nr", "nd", "ng", "ne", "xx", "se", "no", "nb", "nn", "oc", "oj", "or", "om", "os",
    "pi", "fa", "pl", "pt", "pa", "ps", "qu", "rm", "ro", "rn", "ru", "sm", "sg", "sa", "sc", "sr",
    "sh", "sn", "sd", "si", "cu", "sk", "sl", "so", "st", "es", "su", "sw", "ss", "sv", "tl", "ty",
    "tg", "ta", "tt", "te", "th", "bo", "ti", "to", "ts", "tn", "tr", "tk", "tw", "ug", "uk", "ur",
    "uz", "ve", "vi", "vo", "wa", "cy", "wo", "xh", "ii", "yi", "yo", "za", "zu",
];

fn tv_item(i: &Value) -> Option<ListItem> {
    // An absent media type means TV (`TmdbParserBase.AsImportable`).
    if !matches!(i["media_type"].as_str(), None | Some("tv")) {
        return None;
    }
    let id = i["id"].as_i64().filter(|n| *n > 0)?;
    Some(ListItem {
        title: i["name"].as_str().unwrap_or_default().to_string(),
        tmdb_id: Some(id),
        year: i["first_air_date"]
            .as_str()
            .and_then(|d| d.get(..4)?.parse().ok()),
        ..Default::default()
    })
}

/// Up to `max_pages` pages of a paged TMDB endpoint (Sonarr reads 10 pages of discover and
/// account lists).
async fn paged(
    http: &reqwest::Client,
    url: &str,
    token: &str,
    query: &[(String, String)],
    max_pages: i64,
) -> Result<Vec<ListItem>, ListError> {
    let invalid =
        || ListError::Invalid("TMDB returned an invalid page; the previous list is kept".into());
    let mut out = Vec::new();
    for page in 1..=max_pages {
        let resp = http
            .get(url)
            .bearer_auth(token)
            .query(query)
            .query(&[("page", page)])
            .send()
            .await
            .map_err(connection_error)?;
        check_status(&resp)?;
        let body: Value = resp.json().await.map_err(|_| invalid())?;
        let results = body["results"].as_array().ok_or_else(invalid)?;
        out.extend(results.iter().filter_map(tv_item));
        let pages = body["total_pages"].as_i64().unwrap_or(1);
        if page >= pages || results.is_empty() {
            break;
        }
    }
    Ok(out)
}

/// `TmdbDiscoverImport`, `TmdbAccountImport` and `TmdbPersonImport`.
pub async fn fetch_other(
    http: &reqwest::Client,
    implementation: &str,
    s: &Map<String, Value>,
) -> Result<Vec<ListItem>, ListError> {
    let token = text(s, "authToken").ok_or(ListError::Settings(
        "Enter your TMDB API Read Access Token".into(),
    ))?;
    let base = text(s, "baseUrl").unwrap_or_else(|| "https://api.themoviedb.org".into());
    let base = base.trim_end_matches('/');
    let int = |k: &str| s.get(k).and_then(Value::as_i64).unwrap_or(0);
    match implementation {
        "TmdbDiscoverImport" => {
            let sort = [
                "first_air_date.asc",
                "first_air_date.desc",
                "name.asc",
                "name.desc",
                "original_name.asc",
                "original_name.desc",
                "popularity.asc",
                "popularity.desc",
                "vote_average.asc",
                "vote_average.desc",
                "vote_count.asc",
                "vote_count.desc",
            ]
            .get(int("sortByType") as usize)
            .copied()
            .unwrap_or("popularity.desc");
            let mut q: Vec<(String, String)> = vec![
                (
                    "include_null_first_air_dates".into(),
                    crate::flag(s, "includeNullFirstAirDates", false).to_string(),
                ),
                ("sort_by".into(), sort.into()),
            ];
            let language = match s.get("originalLanguage") {
                Some(Value::String(code)) => Some(code.to_lowercase()),
                Some(v) => v
                    .as_i64()
                    .and_then(|i| TMDB_LANGUAGES.get(i as usize))
                    .map(|c| (*c).to_string()),
                None => None,
            };
            if let Some(l) = language.filter(|l| l != "any") {
                q.push(("with_original_language".into(), l));
            }
            let genres = crate::ints(s, "withGenreTypes");
            if !genres.is_empty() {
                let g: Vec<String> = genres.iter().map(i64::to_string).collect();
                q.push(("with_genres".into(), g.join(",")));
            }
            let networks = strings(s, "withNetworks");
            if !networks.is_empty() {
                q.push(("with_networks".into(), networks.join(",")));
            }
            for (k, key) in [
                ("airDateMinimum", "air_date.gte"),
                ("airDateMaximum", "air_date.lte"),
                ("voteAverageMinimum", "vote_average.gte"),
                ("voteCountMinimum", "vote_count.gte"),
                ("withCompanies", "with_companies"),
                ("withKeywords", "with_keywords"),
            ] {
                if let Some(v) = text(s, k) {
                    q.push((key.into(), v));
                }
            }
            paged(http, &format!("{base}/3/discover/tv"), &token, &q, 10).await
        }
        "TmdbAccountImport" => {
            let account = text(s, "accountId")
                .ok_or(ListError::Settings("Enter your TMDB v4 account id".into()))?;
            let kind = match int("accountListType") {
                1 => "rated",
                2 => "recommendations",
                3 => "watchlist",
                _ => "favorites",
            };
            paged(
                http,
                &format!("{base}/4/account/{account}/tv/{kind}"),
                &token,
                &[],
                10,
            )
            .await
        }
        "TmdbPersonImport" => {
            let person = int("personId");
            if person <= 0 {
                return Err(ListError::Settings("Enter the TMDB person id".into()));
            }
            let resp = http
                .get(format!("{base}/3/person/{person}/tv_credits"))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(connection_error)?;
            check_status(&resp)?;
            let body: Value = resp.json().await.map_err(|_| {
                ListError::Invalid("TMDB sent unreadable credits; the previous list is kept".into())
            })?;
            let departments: Vec<&str> = crate::ints(s, "includeDepartmentTypes")
                .iter()
                .filter_map(|d| {
                    [
                        "Art",
                        "Camera",
                        "Costume & Makeup",
                        "Crew",
                        "Directing",
                        "Editing",
                        "Lighting",
                        "Production",
                        "Sound",
                        "Visual Effects",
                        "Writing",
                    ]
                    .get(*d as usize)
                    .copied()
                })
                .collect();
            let mut out = Vec::new();
            if crate::flag(s, "includingCastCredits", true) {
                out.extend(
                    body["cast"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(tv_item),
                );
            }
            out.extend(
                body["crew"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| departments.contains(&c["department"].as_str().unwrap_or_default()))
                    .filter_map(tv_item),
            );
            Ok(out)
        }
        other => Err(ListError::Settings(format!("{other} is not a TMDB list"))),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn language_indexes_follow_tanukarr() {
        assert_eq!(super::TMDB_LANGUAGES.len(), 188);
        assert_eq!(super::TMDB_LANGUAGES[0], "any");
        assert_eq!(super::TMDB_LANGUAGES[1], "ab");
        assert!(super::TMDB_LANGUAGES.contains(&"ja"));
    }
}
