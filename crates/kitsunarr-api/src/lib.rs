//! Kitsunarr HTTP layer (Axum).
//!
//! One router serves:
//! * the native REST API under `/api/v1` (OpenAPI document at `/api/v1/openapi.json`),
//! * Server-Sent Events at `/api/v1/events`,
//! * the compiled Dioxus web application (embedded assets, SPA fallback).
//!
//! Everything is optionally nested under the configured URL base.

pub mod assets;
pub mod auth;
pub mod error;
pub mod logs;
pub mod routes;

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
    pub metadata: kitsunarr_metadata::Metadata,
    pub indexers: kitsunarr_indexers::IndexerClient,
    pub logs: LogBuffer,
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
            http,
            metadata,
            logs,
        }))
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

    let inner = Router::new()
        .merge(api)
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
