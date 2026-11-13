//! Kitsunarr HTTP layer (Axum).
//!
//! One router serves:
//! * the native REST API under `/api/v1` (OpenAPI document at `/api/v1/openapi.json`),
//! * Server-Sent Events at `/api/v1/events`,
//! * the compiled Dioxus web application (embedded assets, SPA fallback).
//!
//! Everything is optionally nested under the configured URL base.

// The Sonarr v3 series resource is one large `json!` literal.
#![recursion_limit = "256"]

pub mod assets;
pub mod auth;
pub mod downloads;
pub mod error;
pub mod health;
pub mod ical;
pub mod import_lists;
pub mod library_tools;
pub mod logs;
pub mod proxy;
pub mod routes;
pub mod rss;
pub mod signalr;
pub mod tls;
pub mod v3;
mod v3_more;
mod v3_providers;
mod v3_settings;

use std::sync::{Arc, RwLock};

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, HeaderValue, header};
use axum::response::Redirect;
use axum::routing::get;
use kitsunarr_config::Config;
use kitsunarr_db::Database;
use kitsunarr_events::EventBus;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::csrf::CsrfLayer;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

pub use assets::{AssetSource, EmbeddedAsset, NoAssets};
pub use logs::LogBuffer;
pub use routes::migration::MigrationSessions;
pub use routes::system::{NoTasks, TaskControl};

/// Build/version information shown in the UI and status endpoint.
#[derive(Debug, Clone)]
pub struct BuildInfo {
    pub version: &'static str,
    pub git_revision: Option<&'static str>,
}

/// Shared application state (cheap to clone).
#[derive(Clone)]
pub struct AppState(Arc<AppStateInner>);

pub struct AppStateInner {
    pub db: Database,
    pub config: Arc<Config>,
    pub events: EventBus,
    pub api_key: RwLock<String>,
    pub login_limiter: auth::LoginLimiter,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub build: BuildInfo,
    pub assets: Arc<dyn AssetSource>,
    pub tasks: Arc<dyn TaskControl>,
    pub migrations: MigrationSessions,
    pub http: reqwest::Client,
    /// The proxy `http` (and the indexer download client) consult per request.
    pub proxy: proxy::HttpProxy,
    /// Sonarr's certificate validation, consulted by every HTTPS request.
    pub certificates: tls::CertificatePolicy,
    /// MyAnimeList sign-in codes Kitsunarr's callback received, by sign-in nonce.
    pub oauth_codes: std::sync::Mutex<std::collections::HashMap<String, String>>,
    pub metadata: kitsunarr_metadata::Metadata,
    pub indexers: kitsunarr_indexers::IndexerClient,
    /// Download clients and completed-download handling (shared with the background job).
    pub downloads: kitsunarr_library::downloads::Monitor,
    /// Recent search results, for grabbing.
    pub releases: routes::grab::ReleaseCache,
    /// The last download client used per protocol (round-robin between equals).
    pub last_download_client: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// Held while a wanted-episodes search runs (one at a time).
    pub wanted_search: Arc<tokio::sync::Mutex<()>>,
    pub logs: LogBuffer,
    /// The warnings and errors of the last health evaluation, for health notifications.
    pub last_health: std::sync::Mutex<Option<Vec<kitsunarr_domain::api::system::HealthCheck>>>,
    /// `RemotePathMappingCheck` after a failed import (`EpisodeImportFailedEvent`), until an
    /// import succeeds.
    pub import_health: std::sync::Mutex<Option<kitsunarr_domain::api::system::HealthCheck>>,
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Database,
        config: Arc<Config>,
        events: EventBus,
        api_key: String,
        build: BuildInfo,
        assets: Arc<dyn AssetSource>,
        tasks: Arc<dyn TaskControl>,
        http: reqwest::Client,
        metadata: kitsunarr_metadata::Metadata,
        logs: LogBuffer,
    ) -> Self {
        Self(Arc::new(AppStateInner {
            db,
            config,
            events,
            api_key: RwLock::new(api_key),
            login_limiter: auth::LoginLimiter::default(),
            started_at: chrono::Utc::now(),
            build,
            assets,
            tasks,
            migrations: MigrationSessions::default(),
            indexers: kitsunarr_indexers::IndexerClient::new(http.clone()),
            downloads: kitsunarr_library::downloads::Monitor::new(http.clone()),
            releases: Default::default(),
            last_download_client: Default::default(),
            wanted_search: Default::default(),
            last_health: Default::default(),
            import_health: Default::default(),
            proxy: Default::default(),
            certificates: Default::default(),
            oauth_codes: Default::default(),
            http,
            metadata,
            logs,
        }))
    }

    /// Shares the download monitor with the background job. Must be called before the state is
    /// shared.
    pub fn with_download_monitor(mut self, monitor: kitsunarr_library::downloads::Monitor) -> Self {
        match Arc::get_mut(&mut self.0) {
            Some(inner) => inner.downloads = monitor,
            None => tracing::warn!("download monitor not replaced: the state is already shared"),
        }
        self
    }

    /// Shares the proxy the HTTP client was built with; the indexer download client (which
    /// does not follow redirects) uses it too. Must be called before the state is shared.
    pub fn with_proxy(mut self, proxy: proxy::HttpProxy) -> Self {
        match Arc::get_mut(&mut self.0) {
            Some(inner) => {
                inner.indexers =
                    std::mem::take(&mut inner.indexers).with_download_proxy(proxy.reqwest());
                inner.proxy = proxy;
            }
            None => tracing::warn!("proxy not applied: the state is already shared"),
        }
        self
    }

    /// Shares the certificate policy the HTTP client was built with; the indexer download
    /// client is rebuilt with it and the proxy. Must be called before the state is shared.
    pub fn with_certificates(mut self, certificates: tls::CertificatePolicy) -> Self {
        match Arc::get_mut(&mut self.0) {
            Some(inner) => {
                let mut builder = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .user_agent(concat!("Kitsunarr/", env!("CARGO_PKG_VERSION")))
                    .proxy(inner.proxy.reqwest());
                if let Some(config) = certificates.rustls_config() {
                    builder = builder.tls_backend_preconfigured(config);
                }
                if let Ok(downloads) = builder.build() {
                    inner.indexers =
                        std::mem::take(&mut inner.indexers).with_download_client(downloads);
                }
                inner.certificates = certificates;
            }
            None => tracing::warn!("certificate policy not applied: the state is already shared"),
        }
        self
    }

    /// Replaces the indexer client (tests use one without the per-indexer request spacing).
    /// Must be called before the state is shared.
    pub fn with_indexer_client(mut self, client: kitsunarr_indexers::IndexerClient) -> Self {
        match Arc::get_mut(&mut self.0) {
            Some(inner) => inner.indexers = client,
            None => tracing::warn!("indexer client not replaced: the state is already shared"),
        }
        self
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Kitsunarr API",
        description = "Native REST API of Kitsunarr. Authenticate with the session cookie (web UI) or an API key \
                       sent as `X-Api-Key` header, `Authorization: Bearer`, or `apikey` query parameter.",
        license(name = "GPL-3.0-or-later", identifier = "GPL-3.0-or-later")
    ),
    servers((url = "/api/v1")),
    tags(
        (name = "auth", description = "Setup, login and API keys"),
        (name = "system", description = "Status, health, tasks and backups"),
        (name = "library", description = "Series, anime, movies and collections"),
        (name = "metadata", description = "Catalogue search, adding titles and metadata refresh"),
        (name = "search", description = "Indexer tests and interactive release search"),
        (name = "queue", description = "Downloads in the download clients"),
        (name = "wanted", description = "Missing and cutoff-unmet episodes, and searching for them"),
        (name = "importlists", description = "Import list sync, membership and exclusions"),
        (name = "settings", description = "Profiles, providers and configuration"),
        (name = "migration", description = "Import from Sonarr and Tanukarr"),
    )
)]
struct ApiDoc;

/// Builds the API router and its OpenAPI document.
pub fn api_router(state: &AppState) -> (Router<AppState>, utoipa::openapi::OpenApi) {
    let public = routes::public_routes();
    let protected = routes::protected_routes().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        auth::require_auth,
    ));
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest("/api/v1", public.merge(protected))
        .split_for_parts()
}

/// The complete application (API + SSE + web UI), ready to serve.
pub fn app(state: AppState) -> Router {
    let (api, openapi) = api_router(&state);
    let openapi_json: Arc<str> = serde_json::to_string(&openapi)
        .unwrap_or_else(|_| "{}".into())
        .into();
    let max_body = state.config.server.max_body_bytes;
    let url_base = state.config.server.url_base.clone();

    // Sonarr's `/api/v3` for third-party tools, behind the same authentication.
    let v3 = v3::router()
        .merge(v3_more::router())
        .merge(v3_settings::router())
        .merge(v3_providers::router())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth_v3,
        ));
    // The few v5 routes the optional compression worker calls back on.
    let v5 = v3::worker_router().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        auth::require_auth_v3,
    ));
    // Sonarr's iCal feed, for calendar apps (API key in the query).
    let feed = ical::feed_router().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        auth::require_auth_v3,
    ));
    // Sonarr's SignalR hub, for tools that follow changes live (Bazarr).
    let signalr = signalr::router().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        auth::require_auth_v3,
    ));
    let inner = Router::new()
        .merge(api)
        .nest("/api/v3", v3)
        .nest("/signalr", signalr)
        .nest("/feed/v3", feed)
        .nest("/api/v5", v5)
        .route(
            "/api/v1/openapi.json",
            get(move || {
                let json = openapi_json.clone();
                async move {
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        json.to_string(),
                    )
                }
            }),
        )
        .fallback(assets::serve)
        .with_state(state)
        .layer(DefaultBodyLimit::max(max_body));

    let app = if url_base.is_empty() {
        inner
    } else {
        let target = format!("{url_base}/");
        Router::new().nest(&url_base, inner).route(
            "/",
            get(move || async move { Redirect::temporary(&target) }),
        )
    };

    app.layer(CsrfLayer::new())
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
            HeaderName::from_static("x-api-key"),
        ]))
        .layer(CatchPanicLayer::new())
}
