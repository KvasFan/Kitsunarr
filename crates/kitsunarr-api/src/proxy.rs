//! The proxy for outgoing requests: Sonarr's `HttpProxySettingsProvider`.
//!
//! The HTTP clients are built once with [`HttpProxy::reqwest`], which looks the settings up for
//! every request, so a saved change applies without a restart. A request is proxied unless its
//! host is in the bypass list (host names with `*` wildcards, IP addresses, CIDR or `a-b`
//! ranges) or is a local address while "bypass local addresses" is on (.NET `WebProxy`'s
//! `BypassProxyOnLocal`: loopback addresses, `localhost` and names without a dot).

use std::net::IpAddr;
use std::sync::{Arc, RwLock};

use kitsunarr_domain::api::settings::{ProxySettings, ProxyType};
use reqwest::Url;

/// The current proxy, shared by every HTTP client.
#[derive(Debug, Clone, Default)]
pub struct HttpProxy(Arc<RwLock<Option<ProxySettings>>>);

impl HttpProxy {
    /// Applies saved settings; a disabled proxy is dropped.
    pub fn set(&self, settings: ProxySettings) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = settings.enabled.then_some(settings);
    }

    /// The enabled proxy's settings.
    pub fn current(&self) -> Option<ProxySettings> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `GetProxySettings(uri)`: the proxy URL for a request, or none to connect directly.
    pub fn proxy_for(&self, url: &Url) -> Option<Url> {
        let guard = self.0.read().unwrap_or_else(|e| e.into_inner());
        let s = guard.as_ref()?;
        if bypassed(s, url) {
            return None;
        }
        proxy_url(s)
    }

    /// The proxy for a `reqwest` client builder.
    pub fn reqwest(&self) -> reqwest::Proxy {
        let me = self.clone();
        reqwest::Proxy::custom(move |url| me.proxy_for(url))
    }
}

/// `http://`, `socks4://` or `socks5h://` (names resolved by the proxy, as .NET's SOCKS5), with
/// the credentials when set. An empty host name is `127.0.0.1`, as upstream.
pub fn proxy_url(s: &ProxySettings) -> Option<Url> {
    let scheme = match s.proxy_type {
        ProxyType::Http => "http",
        ProxyType::Socks4 => "socks4",
        ProxyType::Socks5 => "socks5h",
    };
    let host = match s.hostname.trim() {
        "" => "127.0.0.1",
        h => h,
    };
    let host = match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => format!("[{v6}]"),
        _ => host.to_string(),
    };
    let mut url = Url::parse(&format!("{scheme}://{host}:{}", s.port)).ok()?;
    if !s.username.is_empty() {
        let _ = url.set_username(&s.username);
        let _ = url.set_password(Some(&s.password));
    }
    Some(url)
}

/// `ShouldProxyBeBypassed`.
pub fn bypassed(s: &ProxySettings, url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let ip = host.parse::<IpAddr>().ok();
    if s.bypass_local_addresses && is_local(host, ip) {
        return true;
    }
    let port = url.port_or_known_default();
    s.bypass_filter
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .any(|entry| entry_matches(entry, host, ip, port))
}

fn is_local(host: &str, ip: Option<IpAddr>) -> bool {
    match ip {
        Some(ip) => ip.is_loopback() || ip.is_unspecified(),
        None => host.eq_ignore_ascii_case("localhost") || !host.contains('.'),
    }
}

fn entry_matches(entry: &str, host: &str, ip: Option<IpAddr>, port: Option<u16>) -> bool {
    // `IPAddressRange`: an address, CIDR block or `first-last` range.
    if let Some(ip) = ip
        && let Some(hit) = in_range(entry, ip)
    {
        return hit;
    }
    // A scheme in the entry (`http://*.lan`) is ignored.
    let entry = entry.split_once("://").map_or(entry, |(_, rest)| rest);
    let entry = entry.trim_end_matches('/');
    wildcard(entry, host) || port.is_some_and(|p| wildcard(entry, &format!("{host}:{p}")))
}

/// `Some(contained)` when `entry` is an IP address, CIDR block or range; `None` otherwise.
fn in_range(entry: &str, ip: IpAddr) -> Option<bool> {
    if let Ok(single) = entry.parse::<IpAddr>() {
        return Some(single == ip);
    }
    if let Some((base, bits)) = entry.split_once('/') {
        let base: IpAddr = base.trim().parse().ok()?;
        let bits: u32 = bits.trim().parse().ok()?;
        return Some(match (base, ip) {
            (IpAddr::V4(b), IpAddr::V4(i)) if bits <= 32 => {
                let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
                u32::from(b) & mask == u32::from(i) & mask
            }
            (IpAddr::V6(b), IpAddr::V6(i)) if bits <= 128 => {
                let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
                u128::from(b) & mask == u128::from(i) & mask
            }
            _ => false,
        });
    }
    if let Some((first, last)) = entry.split_once('-') {
        let first: IpAddr = first.trim().parse().ok()?;
        let last: IpAddr = last.trim().parse().ok()?;
        return Some(match (first, last, ip) {
            (IpAddr::V4(f), IpAddr::V4(l), IpAddr::V4(i)) => {
                (u32::from(f)..=u32::from(l)).contains(&u32::from(i))
            }
            (IpAddr::V6(f), IpAddr::V6(l), IpAddr::V6(i)) => {
                (u128::from(f)..=u128::from(l)).contains(&u128::from(i))
            }
            _ => false,
        });
    }
    None
}

/// Case-insensitive match where `*` stands for any run of characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Settings errors, as Sonarr's `HostConfigResourceValidator`: an enabled proxy needs a host
/// name without a scheme or path, and a port.
pub fn validate(s: &ProxySettings) -> Result<(), (&'static str, &'static str)> {
    if !s.enabled {
        return Ok(());
    }
    let host = s.hostname.trim();
    if host.is_empty()
        || host.contains("://")
        || host.contains('/')
        || host.chars().any(char::is_whitespace)
    {
        return Err((
            "hostname",
            "Enter the proxy's host name or IP address, without http:// or a path.",
        ));
    }
    if s.port == 0 {
        return Err(("port", "Enter the proxy's port (1 to 65535)."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(filter: &str, local: bool) -> ProxySettings {
        ProxySettings {
            enabled: true,
            hostname: "proxy.lan".into(),
            port: 3128,
            bypass_filter: filter.into(),
            bypass_local_addresses: local,
            ..Default::default()
        }
    }

    fn url(u: &str) -> Url {
        Url::parse(u).unwrap()
    }

    #[test]
    fn proxy_urls_carry_the_type_and_credentials() {
        let mut s = settings("", true);
        assert_eq!(proxy_url(&s).unwrap().as_str(), "http://proxy.lan:3128/");
        s.proxy_type = ProxyType::Socks5;
        s.username = "me".into();
        s.password = "p@ss".into();
        assert_eq!(
            proxy_url(&s).unwrap().as_str(),
            "socks5h://me:p%40ss@proxy.lan:3128"
        );
        s.proxy_type = ProxyType::Socks4;
        s.hostname = "::1".into();
        s.username.clear();
        assert_eq!(proxy_url(&s).unwrap().as_str(), "socks4://[::1]:3128");
        s.hostname = " ".into();
        assert_eq!(proxy_url(&s).unwrap().as_str(), "socks4://127.0.0.1:3128");
    }

    #[test]
    fn local_addresses_and_the_bypass_list_connect_directly() {
        let s = settings(
            "*.example.org, 192.168.1.0/24, 10.0.0.5-10.0.0.9, nas:8080",
            true,
        );
        for direct in [
            "http://localhost:8989/",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://qbittorrent:8080/",
            "https://api.example.org/x",
            "http://192.168.1.20/",
            "http://10.0.0.7/",
        ] {
            assert!(bypassed(&s, &url(direct)), "{direct}");
        }
        for proxied in [
            "https://api.themoviedb.org/3",
            "https://example.org/",
            "http://192.168.2.20/",
            "http://10.0.0.10/",
            "http://nas.lan:8080/",
        ] {
            assert!(!bypassed(&s, &url(proxied)), "{proxied}");
        }
        // Without "bypass local addresses", a single-label name is proxied unless listed.
        let s = settings("nas:8080", false);
        assert!(!bypassed(&s, &url("http://localhost/")));
        assert!(bypassed(&s, &url("http://nas:8080/")));
        assert!(!bypassed(&s, &url("http://nas:9090/")));
    }

    #[test]
    fn a_disabled_proxy_is_not_used() {
        let proxy = HttpProxy::default();
        assert_eq!(proxy.proxy_for(&url("https://nyaa.si/")), None);
        proxy.set(settings("", true));
        assert_eq!(
            proxy.proxy_for(&url("https://nyaa.si/")).unwrap().as_str(),
            "http://proxy.lan:3128/"
        );
        assert_eq!(proxy.proxy_for(&url("http://localhost/")), None);
        proxy.set(ProxySettings {
            enabled: false,
            ..settings("", true)
        });
        assert_eq!(proxy.proxy_for(&url("https://nyaa.si/")), None);
    }

    #[test]
    fn validation() {
        assert!(validate(&ProxySettings::default()).is_ok());
        assert!(validate(&settings("", true)).is_ok());
        for bad in ["", "http://proxy", "proxy/x", "a b"] {
            let mut s = settings("", true);
            s.hostname = bad.into();
            assert_eq!(validate(&s).unwrap_err().0, "hostname", "{bad}");
        }
        let mut s = settings("", true);
        s.port = 0;
        assert_eq!(validate(&s).unwrap_err().0, "port");
    }

    /// A client built as the server's sends through the proxy saved after it was built.
    #[tokio::test]
    async fn the_shared_client_follows_saved_settings() {
        use std::sync::Mutex;
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = seen.clone();
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(req.uri().to_string());
                "via proxy"
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let proxy = HttpProxy::default();
        let client = reqwest::Client::builder()
            .proxy(proxy.reqwest())
            .build()
            .unwrap();
        proxy.set(ProxySettings {
            enabled: true,
            hostname: "127.0.0.1".into(),
            port: addr.port(),
            ..Default::default()
        });
        let body = client
            .get("http://catalogue.invalid/series?id=1")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "via proxy");
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec!["http://catalogue.invalid/series?id=1".to_string()]
        );
    }
}
