//! Configuration for Kitsunarr.
//!
//! Precedence (lowest → highest): built-in defaults, `<config-dir>/config.toml`,
//! `KITSUNARR_*` environment variables, command-line flags (applied by the server binary).
//!
//! Only process-level settings live here (network, paths, logging, security knobs).
//! Everything users edit in the UI is stored in the database.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const ENV_PREFIX: &str = "KITSUNARR_";
pub const DEFAULT_CONFIG_DIR: &str = "/config";
pub const DEFAULT_DATA_DIR: &str = "/var/lib/kitsunarr";
pub const DEFAULT_PORT: u16 = 8990;
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const DATABASE_FILE_NAME: &str = "kitsunarr.db";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid configuration file {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid value for {key}: {message}")]
    Invalid { key: String, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

impl FromStr for LogFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "text" | "pretty" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!("expected `text` or `json`, got `{other}`")),
        }
    }
}

/// When to mark the session cookie `Secure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CookieSecurity {
    /// Secure when the request arrived over HTTPS (directly or via a trusted proxy).
    #[default]
    Auto,
    Always,
    Never,
}

impl FromStr for CookieSecurity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "always" | "true" => Ok(Self::Always),
            "never" | "false" => Ok(Self::Never),
            other => Err(format!(
                "expected `auto`, `always` or `never`, got `{other}`"
            )),
        }
    }
}

/// An IP network in CIDR notation (`10.0.0.0/8`, `::1/128`) or a single address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpNetwork {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl IpNetwork {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(self.prefix))
                };
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - u32::from(self.prefix))
                };
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            (IpAddr::V6(_), IpAddr::V4(v4)) => self.contains(IpAddr::V6(v4.to_ipv6_mapped())),
            (IpAddr::V4(_), IpAddr::V6(v6)) => v6
                .to_ipv4_mapped()
                .is_some_and(|v4| self.contains(IpAddr::V4(v4))),
        }
    }
}

impl FromStr for IpNetwork {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = addr
            .parse()
            .map_err(|_| format!("`{s}` is not an IP address or CIDR"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("invalid prefix in `{s}`"))?,
            None => max,
        };
        Ok(Self { addr, prefix })
    }
}

impl fmt::Display for IpNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

impl Serialize for IpNetwork {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IpNetwork {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub bind_address: IpAddr,
    pub port: u16,
    /// Path prefix when served behind a reverse proxy, e.g. `/kitsunarr`. Empty for root.
    pub url_base: String,
    /// Proxies whose `X-Forwarded-For` / `X-Forwarded-Proto` headers are trusted.
    pub trusted_proxies: Vec<IpNetwork>,
    /// Maximum request body size for regular API calls (bytes).
    pub max_body_bytes: usize,
    /// Maximum size of an uploaded Sonarr/Tanukarr backup (bytes).
    pub max_backup_upload_bytes: usize,
    pub request_timeout_secs: u64,
    /// Overrides Settings › General › Security › Allowed hosts (comma separated host names).
    pub allowed_hosts: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            url_base: String::new(),
            trusted_proxies: Vec::new(),
            max_body_bytes: 2 * 1024 * 1024,
            max_backup_upload_bytes: 2 * 1024 * 1024 * 1024,
            request_timeout_secs: 60,
            allowed_hosts: None,
        }
    }
}

/// How users sign in (Sonarr's `AuthenticationType`). Set here, it overrides Settings ›
/// General › Security.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// Kitsunarr's sign-in page. Sonarr's retired `basic` and `none` mean this, as in Tanukarr.
    #[serde(alias = "basic", alias = "none")]
    Forms,
    /// A reverse proxy authenticates; Kitsunarr asks for nothing.
    External,
    /// An OpenID Connect identity provider.
    Oidc,
}

impl FromStr for AuthMethod {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "forms" | "basic" | "none" => Ok(Self::Forms),
            "external" => Ok(Self::External),
            "oidc" => Ok(Self::Oidc),
            other => Err(format!(
                "expected `forms`, `external` or `oidc`, got `{other}`"
            )),
        }
    }
}

/// When authentication is required (Sonarr's `AuthenticationRequiredType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthRequired {
    Enabled,
    #[serde(alias = "disabled_for_local_addresses")]
    DisabledForLocalAddresses,
    #[serde(alias = "disabled_for_localhost", alias = "disabledForLocalHost")]
    DisabledForLocalhost,
}

impl FromStr for AuthRequired {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['_', '-'], "")
            .as_str()
        {
            "enabled" => Ok(Self::Enabled),
            "disabledforlocaladdresses" => Ok(Self::DisabledForLocalAddresses),
            "disabledforlocalhost" => Ok(Self::DisabledForLocalhost),
            other => Err(format!(
                "expected `enabled`, `disabledForLocalAddresses` or `disabledForLocalhost`, got `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    /// Session lifetime when "remember me" is selected.
    pub session_days: u32,
    /// Session lifetime otherwise.
    pub short_session_hours: u32,
    pub secure_cookies: CookieSecurity,
    /// Failed logins allowed per client address within `login_window_secs`.
    pub max_failed_logins: u32,
    pub login_window_secs: u64,
    /// Overrides of Settings › General › Security (Tanukarr's `SONARR__AUTH__*` options).
    /// Unset values follow the settings saved in the web interface.
    pub method: Option<AuthMethod>,
    pub required: Option<AuthRequired>,
    pub trust_cgnat_ip_addresses: Option<bool>,
    pub oidc_authority: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_user_identifier: Option<String>,
    pub oidc_scopes: Option<String>,
    /// A password reset token (at least 20 characters) accepted once on the sign-in page's
    /// reset form until the next restart (Tanukarr's `SONARR__AUTH__RESETTOKEN`).
    pub reset_token: Option<String>,
    /// Creates the administrator on first start instead of the setup page
    /// (Tanukarr's `InitialUsername` / `InitialPassword`).
    pub initial_username: Option<String>,
    pub initial_password: Option<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_days: 30,
            short_session_hours: 12,
            secure_cookies: CookieSecurity::Auto,
            max_failed_logins: 10,
            login_window_secs: 300,
            method: None,
            required: None,
            trust_cgnat_ip_addresses: None,
            oidc_authority: None,
            oidc_client_id: None,
            oidc_client_secret: None,
            oidc_user_identifier: None,
            oidc_scopes: None,
            reset_token: None,
            initial_username: None,
            initial_password: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    /// `tracing` filter directive, e.g. `info` or `info,kitsunarr_api=debug`.
    pub level: String,
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: LogFormat::Text,
        }
    }
}

/// Metadata catalogue endpoints. Change them only to route requests through a caching proxy
/// or mirror; the defaults are the public services.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetadataConfig {
    pub tvmaze_url: String,
    pub anilist_url: String,
    /// TheTVDB through SkyHook, Sonarr's proxy.
    pub tvdb_url: String,
    pub tmdb_url: String,
    pub jikan_url: String,
    pub omdb_url: String,
    /// TheXEM, for scene numbering of TheTVDB series.
    pub xem_url: String,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        Self {
            tvmaze_url: "https://api.tvmaze.com".into(),
            anilist_url: "https://graphql.anilist.co".into(),
            tvdb_url: "https://skyhook.sonarr.tv/v2/tvdb".into(),
            tmdb_url: "https://api.themoviedb.org/3".into(),
            jikan_url: "https://api.jikan.moe/v4".into(),
            omdb_url: "https://www.omdbapi.com".into(),
            xem_url: "https://thexem.info".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub log: LogConfig,
    pub metadata: MetadataConfig,
    /// Directory holding `config.toml`.
    #[serde(skip)]
    pub config_dir: PathBuf,
    /// Directory holding the database, backups and logs.
    pub data_dir: PathBuf,
    /// Where backups go (Sonarr's `BackupFolder`); default `<data_dir>/backups`.
    pub backup_folder: Option<PathBuf>,
    /// Keys an environment variable or command-line flag set (`server.port` …), which win
    /// over the file. Not part of the file.
    #[serde(skip)]
    pub overridden: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            auth: AuthConfig::default(),
            log: LogConfig::default(),
            metadata: MetadataConfig::default(),
            config_dir: PathBuf::from(DEFAULT_CONFIG_DIR),
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
            backup_folder: None,
            overridden: Vec::new(),
        }
    }
}

/// Source of environment variables (injectable for tests).
pub trait Env {
    fn var(&self, key: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }
}

impl<const N: usize> Env for [(&str, &str); N] {
    fn var(&self, key: &str) -> Option<String> {
        self.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_string())
    }
}

impl Config {
    /// Loads defaults, then the config file (if present), then environment overrides.
    pub fn load(config_dir_override: Option<&Path>, env: &dyn Env) -> Result<Self, ConfigError> {
        let config_dir = config_dir_override
            .map(Path::to_path_buf)
            .or_else(|| env.var("KITSUNARR_CONFIG_DIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_DIR));
        let mut config = Self::load_file(&config_dir)?;
        config.apply_env(env)?;
        config.normalize()?;
        Ok(config)
    }

    /// Defaults and the config file alone (what the file will give at the next start, before
    /// environment variables and flags).
    pub fn load_file(config_dir: &Path) -> Result<Self, ConfigError> {
        let file = config_dir.join(CONFIG_FILE_NAME);
        let mut config = if file.is_file() {
            let text = std::fs::read_to_string(&file).map_err(|source| ConfigError::Read {
                path: file.clone(),
                source,
            })?;
            toml::from_str::<Config>(&text).map_err(|e| ConfigError::Parse {
                path: file.clone(),
                message: e.to_string(),
            })?
        } else {
            Config::default()
        };
        config.config_dir = config_dir.to_path_buf();
        Ok(config)
    }

    /// Records that an environment variable or flag set `key`.
    pub fn mark_overridden(&mut self, key: &str) {
        if !self.overridden.iter().any(|k| k == key) {
            self.overridden.push(key.to_string());
        }
    }

    fn apply_env(&mut self, env: &dyn Env) -> Result<(), ConfigError> {
        fn parse<T: FromStr>(key: &str, v: String) -> Result<T, ConfigError>
        where
            T::Err: fmt::Display,
        {
            v.trim().parse::<T>().map_err(|e| ConfigError::Invalid {
                key: key.into(),
                message: e.to_string(),
            })
        }
        if let Some(v) = env.var("KITSUNARR_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Some(v) = env.var("KITSUNARR_BIND_ADDRESS") {
            self.server.bind_address = parse("KITSUNARR_BIND_ADDRESS", v)?;
            self.mark_overridden("server.bind_address");
        }
        if let Some(v) = env.var("KITSUNARR_PORT") {
            self.server.port = parse("KITSUNARR_PORT", v)?;
            self.mark_overridden("server.port");
        }
        if let Some(v) = env.var("KITSUNARR_URL_BASE") {
            self.server.url_base = v;
            self.mark_overridden("server.url_base");
        }
        if let Some(v) = env.var("KITSUNARR_BACKUP_DIR") {
            self.backup_folder = Some(PathBuf::from(v));
            self.mark_overridden("backup_folder");
        }
        if let Some(v) = env.var("KITSUNARR_TRUSTED_PROXIES") {
            self.server.trusted_proxies = v
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| parse("KITSUNARR_TRUSTED_PROXIES", s.to_string()))
                .collect::<Result<_, _>>()?;
        }
        if let Some(v) = env.var("KITSUNARR_LOG_LEVEL") {
            self.log.level = v;
            self.mark_overridden("log.level");
        }
        if let Some(v) = env.var("KITSUNARR_LOG_FORMAT") {
            self.log.format = parse("KITSUNARR_LOG_FORMAT", v)?;
        }
        if let Some(v) = env.var("KITSUNARR_SECURE_COOKIES") {
            self.auth.secure_cookies = parse("KITSUNARR_SECURE_COOKIES", v)?;
        }
        if let Some(v) = env.var("KITSUNARR_TVMAZE_URL") {
            self.metadata.tvmaze_url = v;
        }
        if let Some(v) = env.var("KITSUNARR_ANILIST_URL") {
            self.metadata.anilist_url = v;
        }
        if let Some(v) = env.var("KITSUNARR_TVDB_URL") {
            self.metadata.tvdb_url = v;
        }
        if let Some(v) = env.var("KITSUNARR_TMDB_URL") {
            self.metadata.tmdb_url = v;
        }
        if let Some(v) = env.var("KITSUNARR_JIKAN_URL") {
            self.metadata.jikan_url = v;
        }
        if let Some(v) = env.var("KITSUNARR_OMDB_URL") {
            self.metadata.omdb_url = v;
        }
        if let Some(v) = env.var("KITSUNARR_XEM_URL") {
            self.metadata.xem_url = v;
        }
        if let Some(v) = env.var("KITSUNARR_SESSION_DAYS") {
            self.auth.session_days = parse("KITSUNARR_SESSION_DAYS", v)?;
        }
        if let Some(v) = env.var("KITSUNARR_AUTH_METHOD") {
            self.auth.method = Some(parse("KITSUNARR_AUTH_METHOD", v)?);
        }
        if let Some(v) = env.var("KITSUNARR_AUTH_REQUIRED") {
            self.auth.required = Some(parse("KITSUNARR_AUTH_REQUIRED", v)?);
        }
        if let Some(v) = env.var("KITSUNARR_AUTH_TRUST_CGNAT_IP_ADDRESSES") {
            self.auth.trust_cgnat_ip_addresses =
                Some(parse("KITSUNARR_AUTH_TRUST_CGNAT_IP_ADDRESSES", v)?);
        }
        for (key, slot) in [
            (
                "KITSUNARR_AUTH_OIDC_AUTHORITY",
                &mut self.auth.oidc_authority,
            ),
            (
                "KITSUNARR_AUTH_OIDC_CLIENT_ID",
                &mut self.auth.oidc_client_id,
            ),
            (
                "KITSUNARR_AUTH_OIDC_CLIENT_SECRET",
                &mut self.auth.oidc_client_secret,
            ),
            (
                "KITSUNARR_AUTH_OIDC_USER_IDENTIFIER",
                &mut self.auth.oidc_user_identifier,
            ),
            ("KITSUNARR_AUTH_OIDC_SCOPES", &mut self.auth.oidc_scopes),
            ("KITSUNARR_AUTH_RESET_TOKEN", &mut self.auth.reset_token),
            (
                "KITSUNARR_AUTH_INITIAL_USERNAME",
                &mut self.auth.initial_username,
            ),
            (
                "KITSUNARR_AUTH_INITIAL_PASSWORD",
                &mut self.auth.initial_password,
            ),
            ("KITSUNARR_ALLOWED_HOSTS", &mut self.server.allowed_hosts),
        ] {
            if let Some(v) = env.var(key) {
                *slot = Some(v);
            }
        }
        Ok(())
    }

    /// Validates and canonicalises values (URL base shape, port).
    pub fn normalize(&mut self) -> Result<(), ConfigError> {
        self.server.url_base =
            normalize_url_base(&self.server.url_base).map_err(|message| ConfigError::Invalid {
                key: "server.url_base".into(),
                message,
            })?;
        if self.server.port == 0 {
            return Err(ConfigError::Invalid {
                key: "server.port".into(),
                message: "must not be 0".into(),
            });
        }
        for (key, url) in [
            ("metadata.tvmaze_url", &mut self.metadata.tvmaze_url),
            ("metadata.anilist_url", &mut self.metadata.anilist_url),
            ("metadata.tvdb_url", &mut self.metadata.tvdb_url),
            ("metadata.tmdb_url", &mut self.metadata.tmdb_url),
            ("metadata.jikan_url", &mut self.metadata.jikan_url),
            ("metadata.omdb_url", &mut self.metadata.omdb_url),
            ("metadata.xem_url", &mut self.metadata.xem_url),
        ] {
            let trimmed = url.trim().trim_end_matches('/');
            let scheme_ok = trimmed.starts_with("https://") || trimmed.starts_with("http://");
            if !scheme_ok
                || trimmed.contains(char::is_whitespace)
                || trimmed.len() <= "http://".len()
            {
                return Err(ConfigError::Invalid {
                    key: key.into(),
                    message: "must be an http:// or https:// URL".into(),
                });
            }
            *url = trimmed.to_string();
        }
        if self.auth.session_days == 0 || self.auth.short_session_hours == 0 {
            return Err(ConfigError::Invalid {
                key: "auth".into(),
                message: "session lifetimes must be positive".into(),
            });
        }
        Ok(())
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join(DATABASE_FILE_NAME)
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.backup_folder
            .clone()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| self.data_dir.join("backups"))
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE_NAME)
    }

    /// Commented example written next to the database on first start.
    pub fn example_file() -> String {
        format!(
            "# Kitsunarr configuration. Every value can also be set with a KITSUNARR_* environment\n\
             # variable (e.g. KITSUNARR_PORT), which takes precedence over this file.\n\
             # Settings edited in the web interface are stored in the database, not here.\n\
             \n\
             # data_dir = \"{DEFAULT_DATA_DIR}\"\n\
             # backup_folder = \"\"     # default: <data_dir>/backups\n\
             \n\
             [server]\n\
             # bind_address = \"0.0.0.0\"\n\
             # port = {DEFAULT_PORT}\n\
             # url_base = \"\"            # e.g. \"/kitsunarr\" behind a reverse proxy\n\
             # trusted_proxies = []      # e.g. [\"172.16.0.0/12\"]\n\
             \n\
             # allowed_hosts = \"\"       # overrides Settings › General › Security\n\
             \n\
             [auth]\n\
             # session_days = 30\n\
             # secure_cookies = \"auto\"   # auto | always | never\n\
             # Overrides of Settings › General › Security (unset: as saved in the web interface)\n\
             # method = \"forms\"          # forms | external | oidc\n\
             # required = \"enabled\"      # enabled | disabledForLocalAddresses | disabledForLocalhost\n\
             # oidc_authority = \"https://auth.example.com\"\n\
             # oidc_client_id = \"\"\n\
             # oidc_client_secret = \"\"\n\
             # oidc_user_identifier = \"\"  # verified e-mail or subject allowed to sign in\n\
             # oidc_scopes = \"openid profile email\"\n\
             # reset_token = \"\"          # 20+ characters; enables one password reset\n\
             # initial_username = \"admin\"\n\
             # initial_password = \"\"     # creates the administrator on first start\n\
             \n\
             [log]\n\
             # level = \"info\"\n\
             # format = \"text\"           # text | json\n\
             \n\
             [metadata]\n\
             # Catalogue endpoints; change them only to use a caching proxy or mirror.\n\
             # tvmaze_url = \"https://api.tvmaze.com\"\n\
             # anilist_url = \"https://graphql.anilist.co\"\n\
             # tvdb_url = \"https://skyhook.sonarr.tv/v2/tvdb\"\n\
             # tmdb_url = \"https://api.themoviedb.org/3\"\n\
             # jikan_url = \"https://api.jikan.moe/v4\"\n\
             # omdb_url = \"https://www.omdbapi.com\"\n\
             # xem_url = \"https://thexem.info\"\n"
        )
    }
}

/// Settings › General › Host and Logging as written to `config.toml` by the web interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFileValues {
    pub bind_address: IpAddr,
    pub port: u16,
    pub url_base: String,
    pub log_level: String,
    /// `None` removes the key (the default folder).
    pub backup_folder: Option<PathBuf>,
}

/// Log levels the web interface offers (any `tracing` filter is accepted in the file).
pub const LOG_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];

/// Writes the host values into `config.toml`, keeping its comments and other keys, after
/// checking the result loads. Environment variables and flags still win at the next start.
pub fn write_host_values(config_dir: &Path, v: &HostFileValues) -> Result<PathBuf, ConfigError> {
    let file = config_dir.join(CONFIG_FILE_NAME);
    let invalid = |key: &str, message: String| ConfigError::Invalid {
        key: key.into(),
        message,
    };
    if v.port == 0 {
        return Err(invalid("server.port", "must not be 0".into()));
    }
    let url_base = normalize_url_base(&v.url_base).map_err(|m| invalid("server.url_base", m))?;
    let level = v.log_level.trim().to_ascii_lowercase();
    if !LOG_LEVELS.contains(&level.as_str()) {
        return Err(invalid(
            "log.level",
            format!("must be one of {}", LOG_LEVELS.join(", ")),
        ));
    }
    if let Some(folder) = &v.backup_folder
        && !folder.is_absolute()
    {
        return Err(invalid("backup_folder", "must be an absolute path".into()));
    }
    let text = if file.is_file() {
        std::fs::read_to_string(&file).map_err(|source| ConfigError::Read {
            path: file.clone(),
            source,
        })?
    } else {
        String::new()
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| ConfigError::Parse {
                path: file.clone(),
                message: e.to_string(),
            })?;
    match &v.backup_folder {
        Some(folder) => doc["backup_folder"] = toml_edit::value(folder.display().to_string()),
        None => {
            doc.remove("backup_folder");
        }
    }
    let server = doc["server"].or_insert(toml_edit::table());
    server["bind_address"] = toml_edit::value(v.bind_address.to_string());
    server["port"] = toml_edit::value(i64::from(v.port));
    server["url_base"] = toml_edit::value(url_base);
    let log = doc["log"].or_insert(toml_edit::table());
    log["level"] = toml_edit::value(level);
    let out = doc.to_string();
    toml::from_str::<Config>(&out).map_err(|e| ConfigError::Parse {
        path: file.clone(),
        message: e.to_string(),
    })?;
    std::fs::create_dir_all(config_dir).map_err(|source| ConfigError::Read {
        path: config_dir.to_path_buf(),
        source,
    })?;
    let tmp = file.with_extension("toml.tmp");
    std::fs::write(&tmp, out)
        .and_then(|()| std::fs::rename(&tmp, &file))
        .map_err(|source| ConfigError::Read {
            path: file.clone(),
            source,
        })?;
    Ok(file)
}

/// `"kitsunarr/"` → `"/kitsunarr"`, `"/"` → `""`. Rejects characters that would break URLs.
pub fn normalize_url_base(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains("//") || trimmed.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err("must not contain empty, `.` or `..` segments".into());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '~'))
    {
        return Err("may only contain letters, digits, `-`, `_`, `.`, `~` and `/`".into());
    }
    Ok(format!("/{trimmed}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_base_normalisation() {
        assert_eq!(normalize_url_base("").unwrap(), "");
        assert_eq!(normalize_url_base("/").unwrap(), "");
        assert_eq!(normalize_url_base("kitsunarr/").unwrap(), "/kitsunarr");
        assert_eq!(
            normalize_url_base("/apps/kitsunarr").unwrap(),
            "/apps/kitsunarr"
        );
        assert!(normalize_url_base("/a/../b").is_err());
        assert!(normalize_url_base("/a b").is_err());
        assert!(normalize_url_base("/a<script>").is_err());
    }

    #[test]
    fn cidr_matching() {
        let net: IpNetwork = "10.0.0.0/8".parse().unwrap();
        assert!(net.contains("10.1.2.3".parse().unwrap()));
        assert!(!net.contains("11.0.0.1".parse().unwrap()));
        assert!(net.contains("::ffff:10.0.0.1".parse().unwrap()));
        let single: IpNetwork = "192.168.1.5".parse().unwrap();
        assert_eq!(single.prefix, 32);
        assert!(single.contains("192.168.1.5".parse().unwrap()));
        assert!(!single.contains("192.168.1.6".parse().unwrap()));
        let v6: IpNetwork = "fd00::/8".parse().unwrap();
        assert!(v6.contains("fd12::1".parse().unwrap()));
        assert!("10.0.0.0/33".parse::<IpNetwork>().is_err());
    }

    #[test]
    fn file_then_env_precedence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "data_dir = \"/srv/k\"\n[server]\nport = 9000\nurl_base = \"k\"\n[log]\nformat = \"json\"\n",
        )
        .unwrap();
        let env = [
            ("KITSUNARR_PORT", "9100"),
            ("KITSUNARR_TRUSTED_PROXIES", "172.16.0.0/12, 10.0.0.1"),
        ];
        let cfg = Config::load(Some(dir.path()), &env).unwrap();
        assert_eq!(cfg.server.port, 9100, "env overrides file");
        assert_eq!(cfg.server.url_base, "/k");
        assert_eq!(cfg.data_dir, PathBuf::from("/srv/k"));
        assert_eq!(cfg.log.format, LogFormat::Json);
        assert_eq!(cfg.server.trusted_proxies.len(), 2);
        assert_eq!(cfg.database_path(), PathBuf::from("/srv/k/kitsunarr.db"));
    }

    #[test]
    fn rejects_unknown_keys_and_bad_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), "[server]\nprot = 1\n").unwrap();
        assert!(matches!(
            Config::load(Some(dir.path()), &[("X", "y")]),
            Err(ConfigError::Parse { .. })
        ));
        let empty = tempfile::tempdir().unwrap();
        let err = Config::load(Some(empty.path()), &[("KITSUNARR_PORT", "seventy")]).unwrap_err();
        assert!(err.to_string().contains("KITSUNARR_PORT"));
    }

    #[test]
    fn metadata_endpoints_are_validated() {
        let empty = tempfile::tempdir().unwrap();
        let cfg = Config::load(
            Some(empty.path()),
            &[("KITSUNARR_TVMAZE_URL", "http://127.0.0.1:9999/")],
        )
        .unwrap();
        assert_eq!(cfg.metadata.tvmaze_url, "http://127.0.0.1:9999");
        assert_eq!(cfg.metadata.anilist_url, "https://graphql.anilist.co");
        let err = Config::load(
            Some(empty.path()),
            &[("KITSUNARR_ANILIST_URL", "file:///etc")],
        )
        .unwrap_err();
        assert!(err.to_string().contains("metadata.anilist_url"), "{err}");
    }

    #[test]
    fn authentication_overrides_from_file_and_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "[server]\nallowed_hosts = \"kitsunarr.lan\"\n[auth]\nmethod = \"basic\"\nrequired = \"disabledForLocalAddresses\"\noidc_scopes = \"openid email\"\n",
        )
        .unwrap();
        let cfg = Config::load(Some(dir.path()), &[("X", "y")]).unwrap();
        assert_eq!(
            cfg.auth.method,
            Some(AuthMethod::Forms),
            "basic means forms"
        );
        assert_eq!(
            cfg.auth.required,
            Some(AuthRequired::DisabledForLocalAddresses)
        );
        assert_eq!(cfg.server.allowed_hosts.as_deref(), Some("kitsunarr.lan"));
        let env = [
            ("KITSUNARR_AUTH_METHOD", "OIDC"),
            ("KITSUNARR_AUTH_REQUIRED", "disabled_for_localhost"),
            ("KITSUNARR_AUTH_RESET_TOKEN", "abcdefghijklmnopqrstu"),
            ("KITSUNARR_AUTH_OIDC_CLIENT_SECRET", "s3cret"),
            ("KITSUNARR_AUTH_TRUST_CGNAT_IP_ADDRESSES", "true"),
        ];
        let cfg = Config::load(Some(dir.path()), &env).unwrap();
        assert_eq!(cfg.auth.method, Some(AuthMethod::Oidc));
        assert_eq!(cfg.auth.required, Some(AuthRequired::DisabledForLocalhost));
        assert_eq!(
            cfg.auth.reset_token.as_deref(),
            Some("abcdefghijklmnopqrstu")
        );
        assert_eq!(cfg.auth.oidc_client_secret.as_deref(), Some("s3cret"));
        assert_eq!(cfg.auth.oidc_scopes.as_deref(), Some("openid email"));
        assert_eq!(cfg.auth.trust_cgnat_ip_addresses, Some(true));
        let err = Config::load(Some(dir.path()), &[("KITSUNARR_AUTH_METHOD", "ldap")]).unwrap_err();
        assert!(err.to_string().contains("KITSUNARR_AUTH_METHOD"), "{err}");
    }

    #[test]
    fn host_values_are_written_without_losing_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "# my comment\n[server]\nport = 9000\ntrusted_proxies = [\"10.0.0.0/8\"]\n[auth]\nsession_days = 7\n",
        )
        .unwrap();
        let values = HostFileValues {
            bind_address: "127.0.0.1".parse().unwrap(),
            port: 9100,
            url_base: "kitsunarr/".into(),
            log_level: "Debug".into(),
            backup_folder: Some(PathBuf::from("/srv/backups")),
        };
        write_host_values(dir.path(), &values).unwrap();
        let text = std::fs::read_to_string(dir.path().join(CONFIG_FILE_NAME)).unwrap();
        assert!(text.contains("# my comment"), "{text}");
        let cfg = Config::load(Some(dir.path()), &[("X", "y")]).unwrap();
        assert_eq!(cfg.server.port, 9100);
        assert_eq!(cfg.server.url_base, "/kitsunarr");
        assert_eq!(cfg.server.bind_address.to_string(), "127.0.0.1");
        assert_eq!(cfg.server.trusted_proxies.len(), 1);
        assert_eq!(cfg.auth.session_days, 7);
        assert_eq!(cfg.log.level, "debug");
        assert_eq!(cfg.backup_dir(), PathBuf::from("/srv/backups"));
        assert!(cfg.overridden.is_empty());
        // Environment variables still win, and are reported.
        let cfg = Config::load(
            Some(dir.path()),
            &[("KITSUNARR_PORT", "9200"), ("KITSUNARR_LOG_LEVEL", "warn")],
        )
        .unwrap();
        assert_eq!(cfg.server.port, 9200);
        assert_eq!(cfg.log.level, "warn");
        assert_eq!(cfg.overridden, vec!["server.port", "log.level"]);
        // Removing the backup folder restores the default; bad values are refused.
        write_host_values(
            dir.path(),
            &HostFileValues {
                backup_folder: None,
                ..values.clone()
            },
        )
        .unwrap();
        let cfg = Config::load_file(dir.path()).unwrap();
        assert_eq!(cfg.backup_folder, None);
        for bad in [
            HostFileValues {
                port: 0,
                ..values.clone()
            },
            HostFileValues {
                log_level: "loud".into(),
                ..values.clone()
            },
            HostFileValues {
                url_base: "/a b".into(),
                ..values.clone()
            },
            HostFileValues {
                backup_folder: Some(PathBuf::from("relative")),
                ..values.clone()
            },
        ] {
            assert!(write_host_values(dir.path(), &bad).is_err());
        }
        // A new file is created when there is none.
        let empty = tempfile::tempdir().unwrap();
        write_host_values(empty.path(), &values).unwrap();
        assert_eq!(Config::load_file(empty.path()).unwrap().server.port, 9100);
    }

    #[test]
    fn example_file_parses() {
        let cfg: Config = toml::from_str(&Config::example_file()).unwrap();
        assert_eq!(cfg.server.port, DEFAULT_PORT);
    }
}
