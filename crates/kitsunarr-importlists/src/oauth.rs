//! Signing in to list services from the list editor, instead of pasting tokens:
//!
//! - **Simkl**: the PIN flow (`/oauth/pin`), with the list's Client ID;
//! - **Trakt**: the device flow (`/oauth/device/code`, then `/oauth/device/token`), with the
//!   list's Client ID and secret (the user's own Trakt application);
//! - **Plex**: Plex's PIN flow (`plex.tv/api/v2/pins`), as Sonarr's Plex sign-in;
//! - **MyAnimeList**: authorization code with PKCE; MyAnimeList redirects the browser to
//!   Kitsunarr's callback, which keeps the code for the editor's next poll.
//!
//! [`start`] gives the page to open (and a code to enter, when the service shows one) and an
//! opaque state; the editor then calls [`poll`] at the given interval until the service hands
//! over a token, which comes back as settings to merge.

use serde_json::{Map, Value, json};

use crate::{ListError, connection_error, text};

const SIMKL_API: &str = "https://api.simkl.com";
const TRAKT_API: &str = "https://api.trakt.tv";
const PLEX_TV: &str = "https://plex.tv";
const PLEX_APP: &str = "https://app.plex.tv";
const MAL_AUTHORIZE: &str = "https://myanimelist.net/v1/oauth2/authorize";

/// A sign-in in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    /// The page the user opens.
    pub url: String,
    /// The code to enter there (Simkl, Trakt).
    pub code: Option<String>,
    /// Opaque state for [`poll`].
    pub state: String,
    /// Seconds between polls.
    pub interval: u64,
    /// Seconds until the sign-in expires.
    pub expires_in: u64,
}

/// What a poll found.
#[derive(Debug, Clone, PartialEq)]
pub enum Polled {
    /// The user has not finished signing in yet.
    Pending,
    /// The settings to store (the token and what goes with it).
    Done(Map<String, Value>),
}

/// Which implementations can sign in.
pub fn supported(implementation: &str) -> bool {
    matches!(
        implementation,
        "SimklUserImport"
            | "TraktListImport"
            | "TraktUserImport"
            | "TraktPopularImport"
            | "TraktSmartListImport"
            | "PlexImport"
            | "MyAnimeListImport"
    )
}

fn random() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn required(s: &Map<String, Value>, key: &str, what: &str) -> Result<String, ListError> {
    text(s, key).ok_or_else(|| ListError::Settings(format!("Enter {what} first")))
}

async fn json_of(resp: reqwest::Response, service: &str) -> Result<Value, ListError> {
    resp.json()
        .await
        .map_err(|_| ListError::Invalid(format!("{service} sent an unreadable answer")))
}

fn plex_client(s: &Map<String, Value>) -> String {
    text(s, "clientIdentifier").unwrap_or_else(|| "kitsunarr".into())
}

/// Starts signing in. `callback` is Kitsunarr's MyAnimeList redirect URL.
pub async fn start(
    http: &reqwest::Client,
    implementation: &str,
    s: &Map<String, Value>,
    callback: &str,
) -> Result<Started, ListError> {
    match implementation {
        "SimklUserImport" => {
            let client_id = text(s, "clientIdV2")
                .or_else(|| text(s, "clientId"))
                .ok_or_else(|| ListError::Settings("Enter your Simkl Client ID first".into()))?;
            let base = text(s, "baseUrl").unwrap_or_else(|| SIMKL_API.into());
            let resp = http
                .get(format!("{}/oauth/pin", base.trim_end_matches('/')))
                .query(&[("client_id", client_id.as_str())])
                .send()
                .await
                .map_err(connection_error)?;
            crate::check_status(&resp)?;
            let v = json_of(resp, "Simkl").await?;
            let code = v["user_code"]
                .as_str()
                .ok_or_else(|| ListError::Invalid("Simkl sent no code".into()))?
                .to_string();
            Ok(Started {
                url: v["verification_url"]
                    .as_str()
                    .unwrap_or("https://simkl.com/pin")
                    .to_string(),
                code: Some(code.clone()),
                state: json!({"code": code}).to_string(),
                interval: v["interval"].as_u64().unwrap_or(5).max(1),
                expires_in: v["expires_in"].as_u64().unwrap_or(900),
            })
        }
        i if i.starts_with("Trakt") => {
            let client_id = required(s, "clientId", "your Trakt Client ID")?;
            let base = text(s, "baseUrl").unwrap_or_else(|| TRAKT_API.into());
            let resp = http
                .post(format!("{}/oauth/device/code", base.trim_end_matches('/')))
                .json(&json!({"client_id": client_id}))
                .send()
                .await
                .map_err(connection_error)?;
            crate::check_status(&resp)?;
            let v = json_of(resp, "Trakt").await?;
            let (Some(device), Some(code)) = (v["device_code"].as_str(), v["user_code"].as_str())
            else {
                return Err(ListError::Invalid("Trakt sent no device code".into()));
            };
            Ok(Started {
                url: v["verification_url"]
                    .as_str()
                    .unwrap_or("https://trakt.tv/activate")
                    .to_string(),
                code: Some(code.to_string()),
                state: json!({"device": device}).to_string(),
                interval: v["interval"].as_u64().unwrap_or(5).max(1),
                expires_in: v["expires_in"].as_u64().unwrap_or(600),
            })
        }
        "PlexImport" => {
            let client = plex_client(s);
            let base = text(s, "authBaseUrl").unwrap_or_else(|| PLEX_TV.into());
            let resp = http
                .post(format!("{}/api/v2/pins", base.trim_end_matches('/')))
                .query(&[("strong", "true")])
                .header("Accept", "application/json")
                .header("X-Plex-Product", "Kitsunarr")
                .header("X-Plex-Client-Identifier", &client)
                .send()
                .await
                .map_err(connection_error)?;
            crate::check_status(&resp)?;
            let v = json_of(resp, "Plex").await?;
            let (Some(id), Some(code)) = (v["id"].as_i64(), v["code"].as_str()) else {
                return Err(ListError::Invalid("Plex sent no PIN".into()));
            };
            let app = text(s, "authAppUrl").unwrap_or_else(|| PLEX_APP.into());
            Ok(Started {
                url: format!(
                    "{}/auth#?clientID={}&code={}&context%5Bdevice%5D%5Bproduct%5D=Kitsunarr",
                    app.trim_end_matches('/'),
                    urlencode(&client),
                    urlencode(code)
                ),
                code: None,
                state: json!({"pin": id}).to_string(),
                interval: 2,
                expires_in: v["expiresIn"].as_u64().unwrap_or(1800),
            })
        }
        "MyAnimeListImport" => {
            let client_id = required(s, "clientId", "your MyAnimeList Client ID")?;
            // PKCE with the "plain" method (the only one MyAnimeList accepts).
            let verifier = format!("{}{}", random(), random());
            let nonce = random();
            let authorize = text(s, "authorizeUrl").unwrap_or_else(|| MAL_AUTHORIZE.into());
            Ok(Started {
                url: format!(
                    "{authorize}?response_type=code&client_id={}&code_challenge={verifier}&code_challenge_method=plain&state={nonce}&redirect_uri={}",
                    urlencode(&client_id),
                    urlencode(callback)
                ),
                code: None,
                state: json!({"verifier": verifier, "nonce": nonce, "redirect": callback})
                    .to_string(),
                interval: 2,
                expires_in: 600,
            })
        }
        other => Err(ListError::Settings(format!(
            "{other} has no sign-in; enter its token"
        ))),
    }
}

/// The MyAnimeList nonce a sign-in waits for (its callback's `state`).
pub fn nonce_of(state: &str) -> Option<String> {
    serde_json::from_str::<Value>(state).ok()?["nonce"]
        .as_str()
        .map(str::to_string)
}

/// Checks whether the user finished signing in. For MyAnimeList, `code` is what Kitsunarr's
/// callback received for this sign-in's nonce.
pub async fn poll(
    http: &reqwest::Client,
    implementation: &str,
    s: &Map<String, Value>,
    state: &str,
    code: Option<&str>,
) -> Result<Polled, ListError> {
    let st: Value = serde_json::from_str(state)
        .map_err(|_| ListError::Settings("The sign-in state is invalid; start again".into()))?;
    let mut out = Map::new();
    match implementation {
        "SimklUserImport" => {
            let client_id = text(s, "clientIdV2")
                .or_else(|| text(s, "clientId"))
                .ok_or_else(|| ListError::Settings("Enter your Simkl Client ID first".into()))?;
            let base = text(s, "baseUrl").unwrap_or_else(|| SIMKL_API.into());
            let user_code = st["code"].as_str().unwrap_or_default();
            let resp = http
                .get(format!(
                    "{}/oauth/pin/{}",
                    base.trim_end_matches('/'),
                    urlencode(user_code)
                ))
                .query(&[("client_id", client_id.as_str())])
                .send()
                .await
                .map_err(connection_error)?;
            crate::check_status(&resp)?;
            let v = json_of(resp, "Simkl").await?;
            match v["access_token"].as_str() {
                Some(t) if v["result"].as_str() != Some("KO") => {
                    out.insert("accessToken".into(), json!(t));
                }
                _ => return Ok(Polled::Pending),
            }
        }
        i if i.starts_with("Trakt") => {
            let client_id = required(s, "clientId", "your Trakt Client ID")?;
            let secret = required(s, "clientSecret", "your Trakt Client Secret")?;
            let base = text(s, "baseUrl").unwrap_or_else(|| TRAKT_API.into());
            let resp = http
                .post(format!("{}/oauth/device/token", base.trim_end_matches('/')))
                .json(
                    &json!({"code": st["device"], "client_id": client_id, "client_secret": secret}),
                )
                .send()
                .await
                .map_err(connection_error)?;
            match resp.status().as_u16() {
                // Pending, or polling too fast.
                400 | 429 => return Ok(Polled::Pending),
                404 => {
                    return Err(ListError::Settings(
                        "Trakt does not know this code; start again".into(),
                    ));
                }
                409 => {
                    return Err(ListError::Settings(
                        "This Trakt code was already used; start again".into(),
                    ));
                }
                410 => {
                    return Err(ListError::Settings(
                        "The Trakt code expired; start again".into(),
                    ));
                }
                418 => {
                    return Err(ListError::Authentication(
                        "The Trakt sign-in was denied".into(),
                    ));
                }
                _ => {}
            }
            crate::check_status(&resp)?;
            let v = json_of(resp, "Trakt").await?;
            let access = v["access_token"]
                .as_str()
                .ok_or_else(|| ListError::Invalid("Trakt sent no token".into()))?;
            out.insert("accessToken".into(), json!(access));
            if let Some(r) = v["refresh_token"].as_str() {
                out.insert("refreshToken".into(), json!(r));
            }
            if let Some(e) = v["expires_in"].as_i64() {
                out.insert(
                    "expires".into(),
                    json!(crate::myanimelist::format_secs(
                        crate::myanimelist::now_secs() + e
                    )),
                );
            }
        }
        "PlexImport" => {
            let base = text(s, "authBaseUrl").unwrap_or_else(|| PLEX_TV.into());
            let id = st["pin"].as_i64().unwrap_or_default();
            let resp = http
                .get(format!("{}/api/v2/pins/{id}", base.trim_end_matches('/')))
                .header("Accept", "application/json")
                .header("X-Plex-Client-Identifier", plex_client(s))
                .send()
                .await
                .map_err(connection_error)?;
            crate::check_status(&resp)?;
            let v = json_of(resp, "Plex").await?;
            match v["authToken"].as_str().filter(|t| !t.is_empty()) {
                Some(t) => {
                    out.insert("accessToken".into(), json!(t));
                }
                None => return Ok(Polled::Pending),
            }
        }
        "MyAnimeListImport" => {
            let Some(code) = code else {
                return Ok(Polled::Pending);
            };
            let client_id = required(s, "clientId", "your MyAnimeList Client ID")?;
            let mut form = vec![
                ("client_id", client_id),
                ("grant_type", "authorization_code".to_string()),
                ("code", code.to_string()),
                (
                    "code_verifier",
                    st["verifier"].as_str().unwrap_or_default().to_string(),
                ),
                (
                    "redirect_uri",
                    st["redirect"].as_str().unwrap_or_default().to_string(),
                ),
            ];
            if let Some(secret) = text(s, "clientSecret") {
                form.push(("client_secret", secret));
            }
            let url = text(s, "tokenUrl").unwrap_or_else(|| crate::myanimelist::TOKEN_URL.into());
            let resp = http
                .post(url)
                .form(&form)
                .send()
                .await
                .map_err(connection_error)?;
            if !resp.status().is_success() {
                return Err(ListError::Authentication(format!(
                    "MyAnimeList refused the sign-in (HTTP {})",
                    resp.status().as_u16()
                )));
            }
            let v = json_of(resp, "MyAnimeList").await?;
            let access = v["access_token"]
                .as_str()
                .ok_or_else(|| ListError::Invalid("MyAnimeList sent no token".into()))?;
            out.insert("accessToken".into(), json!(access));
            if let Some(r) = v["refresh_token"].as_str() {
                out.insert("refreshToken".into(), json!(r));
            }
            if let Some(e) = v["expires_in"].as_i64() {
                out.insert(
                    "expires".into(),
                    json!(crate::myanimelist::format_secs(
                        crate::myanimelist::now_secs() + e
                    )),
                );
            }
        }
        other => {
            return Err(ListError::Settings(format!(
                "{other} has no sign-in; enter its token"
            )));
        }
    }
    Ok(Polled::Done(out))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
