//! Reading a running Sonarr/Tanukarr instance through its v3 API (read-only GET requests).
//!
//! SSRF protection: only http(s); the host is resolved once and every address is vetted
//! (link-local/metadata, multicast and unspecified addresses are always refused; private and
//! loopback ranges only when the user allows them); the vetted address is pinned for the
//! connection (no DNS rebinding); redirects are not followed; response sizes are capped.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures::StreamExt;
use kitsunarr_domain::api::migration::{ApiSourceRequest, SourceApp};
use serde_json::Value;
use url::Url;

use crate::MigrateError;
use crate::model::{RawSource, SourceFormat};
use crate::values::Rec;

const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;
const MAX_HISTORY_PAGES: u32 = 100;
const HISTORY_PAGE_SIZE: u32 = 1000;
const SERIES_CONCURRENCY: usize = 6;

/// Classifies an address. Returns an error message when it must not be contacted.
pub fn check_ip(ip: IpAddr, allow_private: bool) -> Result<(), String> {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    let (blocked, private) = match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            (
                v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.is_broadcast()
                    || v4.is_link_local()
                    || o[0] == 0,
                v4.is_private() || v4.is_loopback() || (o[0] == 100 && (o[1] & 0xC0) == 64),
            )
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            let link_local = (seg[0] & 0xffc0) == 0xfe80;
            let unique_local = (seg[0] & 0xfe00) == 0xfc00;
            // AWS IMDS over IPv6 lives inside the unique-local range.
            let metadata = seg == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254];
            (
                v6.is_unspecified() || v6.is_multicast() || link_local || metadata,
                v6.is_loopback() || unique_local,
            )
        }
    };
    if blocked {
        return Err(format!(
            "{ip} is a link-local, multicast or otherwise reserved address and is never contacted"
        ));
    }
    if private && !allow_private {
        return Err(format!(
            "{ip} is a private or loopback address; enable private network access to use it"
        ));
    }
    Ok(())
}

pub struct RemoteSource {
    http: reqwest::Client,
    base: Url,
    api_key: String,
}

impl RemoteSource {
    pub async fn connect(req: &ApiSourceRequest, user_agent: &str) -> Result<Self, MigrateError> {
        let mut base = Url::parse(req.url.trim())
            .map_err(|e| MigrateError::InvalidSource(format!("invalid URL: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(MigrateError::InvalidSource(
                "only http:// and https:// URLs are supported".into(),
            ));
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err(MigrateError::InvalidSource(
                "credentials in the URL are not allowed; use the API key field".into(),
            ));
        }
        if base.query().is_some() || base.fragment().is_some() {
            base.set_query(None);
            base.set_fragment(None);
        }
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        let host = base
            .host_str()
            .ok_or_else(|| MigrateError::InvalidSource("the URL has no host".into()))?
            .to_string();
        let port = base.port_or_known_default().unwrap_or(80);
        let key = req.api_key.trim().to_string();
        if key.is_empty() || key.len() > 256 || !key.chars().all(|c| c.is_ascii_graphic()) {
            return Err(MigrateError::InvalidSource(
                "enter the source API key (Settings → General in Sonarr)".into(),
            ));
        }

        let lookup_host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((lookup_host.as_str(), port))
            .await
            .map_err(|e| MigrateError::Unreachable(format!("cannot resolve {host}: {e}")))?
            .collect();
        if addrs.is_empty() {
            return Err(MigrateError::Unreachable(format!(
                "{host} did not resolve to any address"
            )));
        }
        for addr in &addrs {
            check_ip(addr.ip(), req.allow_private_network).map_err(MigrateError::InvalidSource)?;
        }

        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .user_agent(user_agent);
        if base
            .host()
            .is_some_and(|h| matches!(h, url::Host::Domain(_)))
        {
            builder = builder.resolve(&host, addrs[0]);
        }
        let http = builder
            .build()
            .map_err(|e| MigrateError::Unreachable(e.to_string()))?;
        Ok(Self {
            http,
            base,
            api_key: key,
        })
    }

    fn url(&self, path: &str) -> Result<Url, MigrateError> {
        self.base
            .join(path)
            .map_err(|e| MigrateError::InvalidSource(e.to_string()))
    }

    /// GET returning `None` for 404 (endpoint absent in this version).
    pub async fn get_optional(&self, path: &str) -> Result<Option<Value>, MigrateError> {
        let url = self.url(path)?;
        let mut resp = self
            .http
            .get(url.clone())
            .header("X-Api-Key", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                MigrateError::Unreachable(format!("{}: {}", url.path(), describe_reqwest(&e)))
            })?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(MigrateError::InvalidSource(
                "the source rejected the API key".into(),
            ));
        }
        if status.is_redirection() {
            return Err(MigrateError::InvalidSource(format!(
                "the source answered with a redirect ({}); enter the final URL including any URL base",
                resp.headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("?")
            )));
        }
        if !status.is_success() {
            return Err(MigrateError::Unreachable(format!(
                "{} returned HTTP {status}",
                url.path()
            )));
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| MigrateError::Unreachable(describe_reqwest(&e)))?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(MigrateError::Unreachable(format!(
                    "{} returned more than {MAX_RESPONSE_BYTES} bytes",
                    url.path()
                )));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map(Some).map_err(|_| {
            MigrateError::InvalidSource(format!(
                "{} did not return JSON; is this a Sonarr/Tanukarr URL?",
                url.path()
            ))
        })
    }

    pub async fn get(&self, path: &str) -> Result<Value, MigrateError> {
        self.get_optional(path).await?.ok_or_else(|| {
            MigrateError::InvalidSource(format!("{path} is not available on the source"))
        })
    }
}

fn describe_reqwest(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection failed".into()
    } else {
        e.to_string()
    }
}

/// Detected application, from `/api/v3/system/status`.
#[derive(Debug, Clone)]
pub struct RemoteStatus {
    pub app: SourceApp,
    pub version: String,
    pub product_version: Option<String>,
    pub migration_version: Option<i64>,
    pub instance_name: Option<String>,
}

pub fn detect(status: &Value) -> Result<RemoteStatus, MigrateError> {
    let r = Rec::from_value(status.clone());
    let app_name = r.str("appname").unwrap_or_default();
    let product = r.str("productname");
    let app = if product
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case("tanukarr"))
    {
        SourceApp::Tanukarr
    } else if app_name.eq_ignore_ascii_case("sonarr") {
        SourceApp::Sonarr
    } else if app_name.is_empty() {
        return Err(MigrateError::InvalidSource(
            "the URL did not return a Sonarr system status".into(),
        ));
    } else {
        return Err(MigrateError::InvalidSource(format!(
            "{app_name} is not supported; only Sonarr and Tanukarr can be migrated"
        )));
    };
    let version = r.str("version").unwrap_or_default();
    if app == SourceApp::Sonarr
        && version
            .split('.')
            .next()
            .and_then(|m| m.parse::<u32>().ok())
            .is_some_and(|m| m < 3)
    {
        return Err(MigrateError::InvalidSource(format!(
            "Sonarr {version} is too old; upgrade to Sonarr v3 or v4 first"
        )));
    }
    Ok(RemoteStatus {
        app,
        version,
        product_version: r.str("productversion"),
        migration_version: r.i64("migrationversion"),
        instance_name: r.str("instancename"),
    })
}

fn records(v: Value) -> Vec<Rec> {
    match v {
        Value::Array(items) => items.into_iter().map(Rec::from_value).collect(),
        Value::Object(o) if o.contains_key("records") => {
            o.get("records").cloned().map(records).unwrap_or_default()
        }
        Value::Object(o) => vec![Rec::from_map(o)],
        _ => Vec::new(),
    }
}

/// Episodes and episode files of one series.
type SeriesFetch = Result<(Vec<Rec>, Vec<Rec>), MigrateError>;

/// Fetches everything the migration needs. `progress(message, done, total)`.
pub async fn load(
    src: &RemoteSource,
    progress: &(dyn Fn(String, u64, u64) + Send + Sync),
) -> Result<(RemoteStatus, RawSource, Vec<String>), MigrateError> {
    let status = detect(&src.get("api/v3/system/status").await?)?;
    let mut tables: HashMap<String, Vec<Rec>> = HashMap::new();
    let mut notes = Vec::new();
    let resources: &[(&str, &str)] = &[
        ("tags", "api/v3/tag"),
        ("rootfolders", "api/v3/rootfolder"),
        ("qualitydefinitions", "api/v3/qualitydefinition"),
        ("customformats", "api/v3/customformat"),
        ("autotagging", "api/v3/autotagging"),
        ("metadata", "api/v3/metadata"),
        ("qualityprofiles", "api/v3/qualityprofile"),
        ("delayprofiles", "api/v3/delayprofile"),
        ("releaseprofiles", "api/v3/releaseprofile"),
        ("indexers", "api/v3/indexer"),
        ("downloadclients", "api/v3/downloadclient"),
        ("notifications", "api/v3/notification"),
        ("importlists", "api/v3/importlist"),
        ("importlistexclusions", "api/v3/importlistexclusion"),
        ("remotepathmappings", "api/v3/remotepathmapping"),
        ("namingconfig", "api/v3/config/naming"),
        ("mediamanagement", "api/v3/config/mediamanagement"),
        ("indexerconfig", "api/v3/config/indexer"),
        ("hostconfig", "api/v3/config/host"),
        ("downloadclientconfig", "api/v3/config/downloadclient"),
        ("importlistconfig", "api/v3/config/importlist"),
    ];
    let total = resources.len() as u64 + 2;
    for (i, (name, path)) in resources.iter().enumerate() {
        progress(format!("Reading {name}"), i as u64, total);
        match src.get_optional(path).await? {
            Some(v) => {
                tables.insert((*name).to_string(), records(v));
            }
            None => notes.push(format!(
                "The source does not provide {path}; {name} were not imported."
            )),
        }
    }

    progress("Reading series".into(), resources.len() as u64, total);
    let series = records(src.get("api/v3/series").await?);
    let ids: Vec<i64> = series.iter().filter_map(|s| s.i64("id")).collect();
    tables.insert("series".into(), series);

    let count = ids.len() as u64;
    let done = std::sync::atomic::AtomicU64::new(0);
    let results: Vec<SeriesFetch> = futures::stream::iter(ids.iter().copied())
        .map(|id| {
            let done = &done;
            async move {
                let episodes = records(src.get(&format!("api/v3/episode?seriesId={id}")).await?);
                let files = records(
                    src.get(&format!("api/v3/episodefile?seriesId={id}"))
                        .await?,
                );
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n.is_multiple_of(10) || n == count {
                    progress(format!("Read episodes of {n} / {count} series"), n, count);
                }
                Ok((episodes, files))
            }
        })
        .buffer_unordered(SERIES_CONCURRENCY)
        .collect()
        .await;
    let (mut episodes, mut files) = (Vec::new(), Vec::new());
    for r in results {
        let (e, f) = r?;
        episodes.extend(e);
        files.extend(f);
    }
    tables.insert("episodes".into(), episodes);
    tables.insert("episodefiles".into(), files);

    let mut history = Vec::new();
    for page in 1..=MAX_HISTORY_PAGES {
        progress(
            format!("Reading history page {page}"),
            page as u64,
            MAX_HISTORY_PAGES as u64,
        );
        let Some(v) = src
            .get_optional(&format!(
                "api/v3/history?page={page}&pageSize={HISTORY_PAGE_SIZE}&sortKey=date&sortDirection=descending"
            ))
            .await?
        else {
            break;
        };
        let batch = records(v);
        let len = batch.len();
        history.extend(batch);
        if len < HISTORY_PAGE_SIZE as usize {
            break;
        }
        if page == MAX_HISTORY_PAGES {
            notes.push(format!(
                "History was truncated to the newest {} entries.",
                MAX_HISTORY_PAGES * HISTORY_PAGE_SIZE
            ));
        }
    }
    tables.insert("history".into(), history);
    Ok((
        status,
        RawSource {
            format: SourceFormat::Api,
            tables,
        },
        notes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ssrf_classification() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(
            check_ip(ip("169.254.169.254"), true).is_err(),
            "cloud metadata always blocked"
        );
        assert!(check_ip(ip("0.0.0.0"), true).is_err());
        assert!(check_ip(ip("224.0.0.1"), true).is_err());
        assert!(check_ip(ip("fe80::1"), true).is_err());
        assert!(check_ip(ip("fd00:ec2::254"), true).is_err());
        assert!(
            check_ip(ip("::ffff:169.254.169.254"), true).is_err(),
            "mapped addresses are unwrapped"
        );
        assert!(check_ip(ip("192.168.1.10"), true).is_ok());
        assert!(check_ip(ip("192.168.1.10"), false).is_err());
        assert!(check_ip(ip("127.0.0.1"), false).is_err());
        assert!(
            check_ip(ip("100.100.1.1"), false).is_err(),
            "CGNAT/Tailscale counts as private"
        );
        assert!(check_ip(ip("93.184.216.34"), false).is_ok());
    }

    #[test]
    fn detection() {
        let sonarr = detect(&json!({"appName": "Sonarr", "version": "4.0.20.3014", "migrationVersion": 217, "instanceName": "Sonarr"})).unwrap();
        assert_eq!(sonarr.app, SourceApp::Sonarr);
        let tanukarr = detect(&json!({"appName": "Sonarr", "productName": "Tanukarr", "productVersion": "0.1.5", "version": "10.0.0.42907"})).unwrap();
        assert_eq!(tanukarr.app, SourceApp::Tanukarr);
        assert_eq!(tanukarr.product_version.as_deref(), Some("0.1.5"));
        assert!(detect(&json!({"appName": "Radarr", "version": "5.0"})).is_err());
        assert!(detect(&json!({"appName": "Sonarr", "version": "2.0.0.5000"})).is_err());
    }

    #[tokio::test]
    async fn connect_rejects_bad_urls() {
        let req = |url: &str| ApiSourceRequest {
            url: url.into(),
            api_key: "k".into(),
            allow_private_network: false,
        };
        assert!(RemoteSource::connect(&req("ftp://x"), "t").await.is_err());
        assert!(
            RemoteSource::connect(&req("http://user:pw@example.com"), "t")
                .await
                .is_err()
        );
        assert!(
            RemoteSource::connect(&req("http://127.0.0.1:8989"), "t")
                .await
                .is_err(),
            "loopback needs opt-in"
        );
        assert!(
            RemoteSource::connect(&req("http://169.254.169.254/"), "t")
                .await
                .is_err()
        );
    }
}
