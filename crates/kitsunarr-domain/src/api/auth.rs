use serde::{Deserialize, Serialize};

pub const MIN_PASSWORD_LENGTH: usize = 10;
pub const MAX_PASSWORD_LENGTH: usize = 1024;
pub const MAX_USERNAME_LENGTH: usize = 64;

/// Returned by `GET /api/v1/auth/status`; available without authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// No administrator exists yet: the first-run setup page must be shown.
    pub setup_required: bool,
    pub authenticated: bool,
    pub username: Option<String>,
    /// How users sign in (the sign-in page offers the identity provider for `oidc`).
    #[serde(default)]
    pub authentication_method: AuthenticationMethod,
    /// OIDC is chosen but its settings are incomplete: password sign-in stays available.
    #[serde(default)]
    pub oidc_misconfigured: bool,
}

/// How users sign in (Sonarr's `AuthenticationType`; Tanukarr's choices). Sonarr's retired
/// `basic` and `none` are read as `forms`, as Tanukarr does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AuthenticationMethod {
    /// Kitsunarr's sign-in page with the administrator's password.
    #[default]
    Forms,
    /// A reverse proxy in front of Kitsunarr authenticates users.
    External,
    /// An OpenID Connect identity provider.
    Oidc,
}

impl AuthenticationMethod {
    /// Sonarr's names (`forms`, `external`, `oidc`; `basic` and `none` mean `forms`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "forms" | "none" => Some(Self::Forms),
            "external" => Some(Self::External),
            "oidc" => Some(Self::Oidc),
            _ => None,
        }
    }
}

/// When authentication is required (Sonarr's `AuthenticationRequiredType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum AuthenticationRequired {
    #[default]
    Enabled,
    /// Requests from private and link-local addresses need no sign-in.
    DisabledForLocalAddresses,
    /// Requests from this machine need no sign-in.
    DisabledForLocalhost,
}

impl AuthenticationRequired {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "enabled" => Some(Self::Enabled),
            "disabledforlocaladdresses" => Some(Self::DisabledForLocalAddresses),
            "disabledforlocalhost" => Some(Self::DisabledForLocalhost),
            _ => None,
        }
    }
}

/// The default OIDC scopes (Tanukarr's).
pub const DEFAULT_OIDC_SCOPES: &str = "openid profile email";

/// Settings › General › Security: authentication, allowed hosts and the OIDC provider
/// (Tanukarr's host settings). `GET /api/v1/settings/security`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct SecuritySettings {
    pub authentication_method: AuthenticationMethod,
    pub authentication_required: AuthenticationRequired,
    /// Host names Kitsunarr answers to, comma separated; empty accepts any host.
    pub allowed_hosts: String,
    /// With "disabled for local addresses", CGNAT addresses (100.64.0.0/10) count as local.
    pub trust_cgnat_ip_addresses: bool,
    /// The provider's address (`https://`), whose `/.well-known/openid-configuration` is read.
    pub oidc_authority: String,
    pub oidc_client_id: String,
    /// Returned redacted.
    pub oidc_client_secret: String,
    /// The verified e-mail address or subject (`sub`) allowed to sign in.
    pub oidc_user_identifier: String,
    /// Space or comma separated; must include `openid`.
    pub oidc_scopes: String,
    /// Read only: the settings `config.toml` or the environment set, which take precedence.
    pub locked: Vec<String>,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            authentication_method: AuthenticationMethod::Forms,
            authentication_required: AuthenticationRequired::Enabled,
            allowed_hosts: String::new(),
            trust_cgnat_ip_addresses: false,
            oidc_authority: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret: String::new(),
            oidc_user_identifier: String::new(),
            oidc_scopes: DEFAULT_OIDC_SCOPES.into(),
            locked: Vec::new(),
        }
    }
}

impl SecuritySettings {
    /// `GetOidcScopes`: split on spaces and commas.
    pub fn oidc_scopes(&self) -> Vec<&str> {
        self.oidc_scopes
            .split([' ', ','])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// `HasOidcSettings`: every OIDC setting is filled in and the scopes include `openid`.
    pub fn has_oidc_settings(&self) -> bool {
        !self.oidc_authority.trim().is_empty()
            && !self.oidc_client_id.trim().is_empty()
            && !self.oidc_client_secret.trim().is_empty()
            && !self.oidc_user_identifier.trim().is_empty()
            && self.oidc_scopes().contains(&"openid")
    }

    /// `IsOidcAuthoritySecure`: `https://`. Kitsunarr also accepts `http://` to this machine
    /// (`localhost`, `127.0.0.0/8`, `[::1]`), which never leaves it.
    pub fn is_oidc_authority_secure(&self) -> bool {
        is_secure_authority(&self.oidc_authority)
    }

    /// `IsOidcConfigured`.
    pub fn is_oidc_configured(&self) -> bool {
        self.has_oidc_settings() && self.is_oidc_authority_secure()
    }

    /// `EffectiveAuthenticationMethod`: OIDC that is not configured falls back to forms.
    pub fn effective_method(&self) -> AuthenticationMethod {
        match self.authentication_method {
            AuthenticationMethod::Oidc if !self.is_oidc_configured() => AuthenticationMethod::Forms,
            m => m,
        }
    }
}

/// `https://…`, or `http://` to the loopback interface.
pub fn is_secure_authority(authority: &str) -> bool {
    let a = authority.trim();
    if a.len() > "https://".len()
        && a.get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
    {
        return true;
    }
    let Some(rest) = a
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("http://"))
        .map(|_| &a[7..])
    else {
        return false;
    };
    let authority_part = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = if let Some(stripped) = authority_part.strip_prefix('[') {
        stripped.split(']').next().unwrap_or_default()
    } else {
        authority_part.split(':').next().unwrap_or_default()
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Tanukarr's `AllowedHostsParser.Parse`: comma or semicolon separated, trimmed, IP addresses
/// in URL form (`[::1]`), without duplicates.
pub fn parse_allowed_hosts(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for host in value
        .split([',', ';'])
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        let host = match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V6(v6)) => format!("[{v6}]"),
            Ok(ip) => ip.to_string(),
            Err(_) => host.to_string(),
        };
        if !out.contains(&host) {
            out.push(host);
        }
    }
    out
}

/// `AllowedHostsParser.IsBindAddress`.
pub fn is_bind_address(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "[::]")
}

/// `AllowedHostsParser.IsValidHost`: a DNS name, an IP address or `*.` and a DNS name.
pub fn is_valid_host(host: &str) -> bool {
    fn dns(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 255
            && name.trim_end_matches('.').split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
    }
    if host.trim().is_empty() {
        return false;
    }
    if let Some(rest) = host.strip_prefix("*.") {
        return dns(rest) && rest.parse::<std::net::IpAddr>().is_err();
    }
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse::<std::net::IpAddr>().is_ok() || dns(host)
}

/// The password reset form (Sonarr's `ResetPasswordResource`), also the first-run setup form.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResetPasswordForm {
    pub token: String,
    pub username: String,
    pub password: String,
    pub password_confirmation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub remember_me: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyResponse {
    pub api_key: String,
}

/// Validates credentials for setup and password changes. Returns a user-facing message.
pub fn validate_credentials(username: &str, password: &str) -> Result<(), String> {
    let username = username.trim();
    if username.is_empty() {
        return Err("Username is required.".into());
    }
    if username.chars().count() > MAX_USERNAME_LENGTH {
        return Err(format!(
            "Username must be at most {MAX_USERNAME_LENGTH} characters."
        ));
    }
    if username.chars().any(|c| c.is_control()) {
        return Err("Username contains invalid characters.".into());
    }
    validate_password(password)
}

pub fn validate_password(password: &str) -> Result<(), String> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LENGTH {
        return Err(format!(
            "Password must be at least {MIN_PASSWORD_LENGTH} characters."
        ));
    }
    if password.len() > MAX_PASSWORD_LENGTH {
        return Err("Password is too long.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_validation() {
        assert!(validate_credentials("admin", "correct horse battery").is_ok());
        assert!(validate_credentials("", "correct horse battery").is_err());
        assert!(validate_credentials("admin", "short").is_err());
        assert!(validate_credentials("ad\nmin", "correct horse battery").is_err());
    }

    #[test]
    fn allowed_hosts_follow_tanukarr_parser() {
        assert_eq!(
            parse_allowed_hosts(" kitsunarr.lan, 192.168.1.5;::1 ,kitsunarr.lan,, "),
            vec!["kitsunarr.lan", "192.168.1.5", "[::1]"]
        );
        assert!(is_valid_host("kitsunarr"));
        assert!(is_valid_host("*.example.com"));
        assert!(is_valid_host("[::1]"));
        assert!(is_valid_host("my_container"));
        assert!(!is_valid_host("http://kitsunarr"));
        assert!(!is_valid_host("kitsunarr:8990"));
        assert!(!is_valid_host("*.1.2.3.4"));
        assert!(is_bind_address("0.0.0.0") && is_bind_address("[::]"));
    }

    #[test]
    fn oidc_configuration_rules() {
        let mut s = SecuritySettings {
            authentication_method: AuthenticationMethod::Oidc,
            oidc_authority: "https://idp.example".into(),
            oidc_client_id: "kitsunarr".into(),
            oidc_client_secret: "secret".into(),
            oidc_user_identifier: "fox@example.com".into(),
            ..Default::default()
        };
        assert!(s.is_oidc_configured());
        assert_eq!(s.effective_method(), AuthenticationMethod::Oidc);
        s.oidc_scopes = "profile, email".into();
        assert!(!s.has_oidc_settings(), "openid is required");
        assert_eq!(s.effective_method(), AuthenticationMethod::Forms);
        s.oidc_scopes = "openid,email".into();
        s.oidc_authority = "http://idp.example".into();
        assert!(!s.is_oidc_configured(), "plain HTTP to another machine");
        for local in [
            "http://localhost:9000",
            "http://127.0.0.1:5556/dex",
            "http://[::1]/",
        ] {
            assert!(is_secure_authority(local), "{local}");
        }
        assert!(!is_secure_authority("http://localhost.evil.example"));
        assert_eq!(
            AuthenticationMethod::parse("None"),
            Some(AuthenticationMethod::Forms)
        );
        assert_eq!(AuthenticationMethod::parse("basic"), None);
    }
}
