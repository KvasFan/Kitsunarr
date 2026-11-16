//! Shared, bounded source queries for TV/anime and movies. Diagnostics expose generated
//! query arguments only, never request URLs, credentials, headers or POST bodies.
use kitsunarr_domain::api::search::IndexerSearchDiagnostic;
use kitsunarr_indexers::request::Chain;
use kitsunarr_indexers::{Criteria, Indexer, IndexerClient, IndexerError, IndexerRelease, Search};
use std::time::{Duration, Instant};

pub(crate) struct SourceOutcome {
    pub result: Result<Vec<(usize, IndexerRelease)>, IndexerError>,
    pub diagnostic: IndexerSearchDiagnostic,
}

pub(crate) fn redact(indexer: &Indexer, message: &str) -> String {
    let settings = &indexer.settings;
    let mut secrets: Vec<String> = [
        settings.api_key.as_deref(),
        settings.passkey.as_deref(),
        settings.username.as_deref(),
        settings.cookie.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|v| !v.is_empty())
    .map(str::to_string)
    .collect();
    if let Some(cookie) = &settings.cookie {
        secrets.extend(
            cookie
                .split(';')
                .filter_map(|p| p.trim().split_once('=').map(|(_, v)| v.to_string()))
                .filter(|v| !v.is_empty()),
        );
    }
    for text in [
        &settings.base_url,
        &format!(
            "https://redacted.invalid/?{}",
            settings
                .additional_parameters
                .as_deref()
                .unwrap_or_default()
                .trim_start_matches(['&', '?'])
        ),
    ] {
        if let Ok(url) = url::Url::parse(text) {
            for credential in [url.username(), url.password().unwrap_or_default()] {
                if !credential.is_empty() {
                    secrets.push(credential.into());
                    // URL userinfo is percent-encoded. Preserve literal plus signs while
                    // collecting the decoded form an upstream error could reflect.
                    let pair = format!("value={}", credential.replace('+', "%2B"));
                    if let Some((_, decoded)) = url::form_urlencoded::parse(pair.as_bytes()).next()
                    {
                        secrets.push(decoded.into_owned());
                    }
                }
            }
            for (key, value) in url.query_pairs() {
                let key = key.to_ascii_lowercase();
                if ["key", "token", "password", "passkey", "auth"]
                    .iter()
                    .any(|word| key.contains(word))
                    && !value.is_empty()
                {
                    secrets.push(value.into_owned());
                }
            }
        }
    }
    // Reflected errors may use the encoded form of a credential.
    let encoded: Vec<String> = secrets
        .iter()
        .flat_map(|value| {
            [
                kitsunarr_indexers::request::escape_data_string(value),
                kitsunarr_indexers::request::url_encode(value),
            ]
        })
        .collect();
    secrets.extend(encoded);
    secrets.sort_by_key(|v| std::cmp::Reverse(v.len()));
    secrets
        .into_iter()
        .fold(message.to_string(), |text, secret| {
            text.replace(&secret, "<redacted>")
        })
}

fn query_plans(indexer: &Indexer, chain: &Chain) -> Vec<String> {
    let mut plans = Vec::new();
    for (tier, queries) in chain.tiers.iter().enumerate() {
        for query in queries {
            let Some(request) = query.requests().into_iter().next() else {
                continue;
            };
            let mut arguments = Vec::new();
            if let Ok(url) = url::Url::parse(&request.url) {
                for (key, value) in url.query_pairs() {
                    if [
                        "t", "q", "term", "season", "ep", "tvdbid", "imdbid", "rid", "tvmazeid",
                        "tmdbid",
                    ]
                    .contains(&key.to_ascii_lowercase().as_str())
                    {
                        arguments.push(format!("{key}={value}"));
                    }
                }
            }
            if let Some(term) = request
                .body
                .as_deref()
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v["search"].as_str().map(str::to_string))
            {
                arguments.push(format!("search={term}"));
            }
            if arguments.is_empty() {
                arguments.push("source-specific query".into());
            }
            let summary = redact(
                indexer,
                &format!("Tier {}: {}", tier + 1, arguments.join(", ")),
            );
            let summary: String = summary.chars().take(512).collect();
            if !plans.contains(&summary) {
                plans.push(summary);
            }
            if plans.len() >= 128 {
                return plans;
            }
        }
    }
    plans
}

pub(crate) async fn query_source(
    client: &IndexerClient,
    indexer: &Indexer,
    criteria: &Criteria,
    searches: &[Search],
    budget: Duration,
) -> SourceOutcome {
    let started = Instant::now();
    let mut plans = Vec::new();
    let work = async {
        let caps = client.capabilities(indexer).await?;
        let generator = client.generator(indexer, &caps);
        let mut found = Vec::new();
        for (i, search) in searches.iter().enumerate() {
            let chain = generator.search(criteria, search);
            for plan in query_plans(indexer, &chain) {
                if plans.len() < 128 && !plans.contains(&plan) {
                    plans.push(plan);
                }
            }
            found.extend(
                client
                    .fetch(indexer, &chain, false)
                    .await?
                    .into_iter()
                    .map(|r| (i, r)),
            );
        }
        Ok(found)
    };
    let (result, timed_out) = match tokio::time::timeout(budget, work).await {
        Ok(result) => (result, false),
        Err(_) => (
            Err(IndexerError::Connection(
                "The source search exceeded its time budget.".into(),
            )),
            true,
        ),
    };
    let diagnostic = IndexerSearchDiagnostic {
        indexer_id: indexer.id,
        indexer: indexer.name.clone(),
        status: if timed_out {
            "timedOut"
        } else if result.is_ok() {
            "success"
        } else {
            "failed"
        }
        .into(),
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        releases: result.as_ref().map_or(0, Vec::len),
        planned_queries: plans,
        notes: if indexer.dialect == kitsunarr_indexers::Dialect::Nyaa {
            vec!["Nyaa RSS returns only the first page of recent results for each query. Page parameters do not retrieve older results; refine the title or episode search.".into()]
        } else {
            Vec::new()
        },
    };
    SourceOutcome { result, diagnostic }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kitsunarr_indexers::{Dialect, SeriesIds, Settings};

    fn indexer(id: i64, base_url: String) -> Indexer {
        Indexer {
            id,
            name: format!("Source {id}"),
            dialect: Dialect::Nyaa,
            settings: Settings {
                base_url,
                anime_standard_format_search: true,
                ..Default::default()
            },
        }
    }
    fn criteria() -> Criteria {
        Criteria {
            ids: SeriesIds::default(),
            scene_titles: vec!["Example Show".into()],
            search_mode: 0,
            anime: false,
        }
    }
    #[test]
    fn plans_redact_secrets_and_do_not_include_request_urls() {
        let mut source = indexer(
            1,
            "https://url%2Buser:password%40value@example.invalid/?token=token-value".into(),
        );
        source.settings.api_key = Some("api-secret".into());
        source.settings.cookie = Some("session=cookie-secret; other=another-secret".into());
        source.settings.username = Some("user-secret".into());
        source.settings.passkey = Some("pass-secret".into());
        source.settings.additional_parameters = Some("&apikey=extra-secret".into());
        let secrets = [
            "url+user",
            "password@value",
            "url%2Buser",
            "password%40value",
            "token-value",
            "api-secret",
            "cookie-secret",
            "another-secret",
            "user-secret",
            "pass-secret",
            "extra-secret",
        ];
        let message = redact(&source, &secrets.join(" "));
        assert!(
            !secrets.iter().any(|secret| message.contains(secret)),
            "{message}"
        );
        let client = IndexerClient::new(reqwest::Client::new());
        let mut c = criteria();
        c.scene_titles = vec!["Example api-secret cookie-secret".into()];
        let chain = client
            .generator(&source, &Default::default())
            .search(&c, &Search::Season { season: 1 });
        let plans = query_plans(&source, &chain);
        assert!(!plans.is_empty());
        let text = plans.join(" ");
        assert!(text.contains("Example"));
        assert!(!text.contains("example.invalid") && !text.contains("https://"));
        assert!(
            !secrets.iter().any(|secret| text.contains(secret)),
            "{text}"
        );
    }
    #[tokio::test]
    async fn source_budget_preserves_other_sources_and_bounds_multiple_queries() {
        let app = axum::Router::new()
            .route(
                "/fast/",
                axum::routing::get(|| async {
                    (
                        [("content-type", "application/rss+xml")],
                        "<rss><channel/></rss>",
                    )
                }),
            )
            .route(
                "/slow/",
                axum::routing::get(|| async { std::future::pending::<&'static str>().await }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = IndexerClient::new(reqwest::Client::new());
        let c = criteria();
        let fast = indexer(1, format!("{base}/fast"));
        let slow = indexer(2, format!("{base}/slow"));
        let searches = [Search::Season { season: 1 }, Search::Season { season: 2 }];
        let one = [Search::Season { season: 1 }];
        let started = Instant::now();
        let (fast, slow) = tokio::join!(
            query_source(&client, &fast, &c, &one, Duration::from_millis(500)),
            query_source(&client, &slow, &c, &searches, Duration::from_millis(30))
        );
        assert!(fast.result.is_ok(), "{:?}", fast.result);
        assert_eq!(fast.diagnostic.status, "success");
        assert!(slow.result.is_err());
        assert_eq!(slow.diagnostic.status, "timedOut");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!slow.diagnostic.planned_queries.is_empty());
        server.abort();
    }
}
