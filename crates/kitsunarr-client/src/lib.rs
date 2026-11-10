//! Typed client for the Kitsunarr REST API.
//!
//! Compiles natively (tests, tooling) and for WebAssembly, where reqwest uses the browser's
//! `fetch` (cookies are sent automatically for same-origin requests).

use kitsunarr_domain::Page;
use kitsunarr_domain::api::API_PREFIX;
use kitsunarr_domain::api::auth::{
    ApiKeyResponse, AuthStatus, ChangePasswordRequest, LoginRequest, SetupRequest,
};
use kitsunarr_domain::api::error::ApiErrorBody;
use kitsunarr_domain::api::library::{
    BulkEdit, BulkEditResult, CalendarEntry, CollectionItem, EpisodeItem, EpisodesMonitoredUpdate,
    HistoryItem, LibraryQuery, LibraryStats, MovieDetail, MovieListItem, SeriesDetail,
    SeriesListItem, SeriesUpdate,
};
use kitsunarr_domain::api::metadata::{
    AddTitleRequest, CatalogueInfo, LibraryRef, RefreshResult, SearchItem, SearchQuery,
};
use kitsunarr_domain::api::migration::{
    ApiSourceRequest, MigrationOptions, MigrationPreview, MigrationReport, MigrationRunItem,
    MigrationSession,
};
use kitsunarr_domain::api::search::{
    DownloadClientTestResult, GrabRequest, GrabResult, IndexerTestRequest, IndexerTestResult,
    ReleaseQuery, ReleaseSearchResult,
};
use kitsunarr_domain::api::settings::{
    CustomFormat, GeneralSettings, IndexerOptionsSettings, MediaManagementSettings, NamingSettings,
    ProviderItem, QualityDefinition, QualityProfile, RootFolderCreate, RootFolderItem, TagCreate,
    TagItem,
};
use kitsunarr_domain::api::system::{
    BackupItem, HealthCheck, ScheduledTaskItem, SystemStatus, TaskRunItem,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    #[error("{}", .body.message)]
    Api { status: u16, body: ApiErrorBody },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Decode(String),
}

impl ClientError {
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn is_unauthorized(&self) -> bool {
        self.status() == Some(401)
    }
}

pub type ClientResult<T> = Result<T, ClientError>;

/// Event names sent by `GET /api/v1/events`.
pub const EVENT_NAMES: &[&str] = &[
    "libraryChanged",
    "task",
    "migration",
    "healthChanged",
    "settingsChanged",
    "notice",
    "resync",
];

#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    /// Absolute origin plus URL base, without trailing slash (e.g. `http://host:8990/kitsunarr`).
    base: String,
    api_key: Option<String>,
}

#[derive(Serialize)]
struct Empty {}

impl Client {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            http: build_http(),
            base: base.into().trim_end_matches('/').to_string(),
            api_key: None,
        }
    }

    /// Native clients usually authenticate with an API key.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{API_PREFIX}{path}", self.base)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self.http.request(method, self.url(path));
        if let Some(key) = &self.api_key {
            req = req.header(kitsunarr_domain::api::API_KEY_HEADER, key);
        }
        req
    }

    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> ClientResult<T> {
        let resp = req
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        if !status.is_success() {
            let body =
                serde_json::from_str::<ApiErrorBody>(&text).unwrap_or_else(|_| ApiErrorBody {
                    code: "http_error".into(),
                    message: format!("The server answered {status}."),
                    fields: Vec::new(),
                });
            return Err(ClientError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let text = if text.trim().is_empty() {
            "null".to_string()
        } else {
            text
        };
        serde_json::from_str(&text).map_err(|e| ClientError::Decode(e.to_string()))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> ClientResult<T> {
        self.send(self.request(reqwest::Method::GET, path)).await
    }

    pub async fn get_query<T: DeserializeOwned, Q: Serialize + ?Sized>(
        &self,
        path: &str,
        query: &Q,
    ) -> ClientResult<T> {
        self.send(self.request(reqwest::Method::GET, path).query(query))
            .await
    }

    pub async fn post<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> ClientResult<T> {
        self.send(self.request(reqwest::Method::POST, path).json(body))
            .await
    }

    pub async fn put<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> ClientResult<T> {
        self.send(self.request(reqwest::Method::PUT, path).json(body))
            .await
    }

    pub async fn patch<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> ClientResult<T> {
        self.send(self.request(reqwest::Method::PATCH, path).json(body))
            .await
    }

    pub async fn delete(&self, path: &str) -> ClientResult<()> {
        self.send::<serde_json::Value>(self.request(reqwest::Method::DELETE, path))
            .await
            .map(|_| ())
    }

    // --- auth ---------------------------------------------------------------------------------
    pub async fn auth_status(&self) -> ClientResult<AuthStatus> {
        self.get("/auth/status").await
    }
    pub async fn setup(&self, req: &SetupRequest) -> ClientResult<AuthStatus> {
        self.post("/auth/setup", req).await
    }
    pub async fn login(&self, req: &LoginRequest) -> ClientResult<AuthStatus> {
        self.post("/auth/login", req).await
    }
    pub async fn logout(&self) -> ClientResult<()> {
        self.send::<serde_json::Value>(
            self.request(reqwest::Method::POST, "/auth/logout")
                .json(&Empty {}),
        )
        .await
        .map(|_| ())
    }
    pub async fn change_password(&self, req: &ChangePasswordRequest) -> ClientResult<()> {
        self.send::<serde_json::Value>(
            self.request(reqwest::Method::POST, "/auth/password")
                .json(req),
        )
        .await
        .map(|_| ())
    }
    pub async fn api_key(&self) -> ClientResult<ApiKeyResponse> {
        self.get("/auth/apikey").await
    }
    pub async fn regenerate_api_key(&self) -> ClientResult<ApiKeyResponse> {
        self.post("/auth/apikey/regenerate", &Empty {}).await
    }

    // --- system -------------------------------------------------------------------------------
    pub async fn system_status(&self) -> ClientResult<SystemStatus> {
        self.get("/system/status").await
    }
    pub async fn health(&self) -> ClientResult<Vec<HealthCheck>> {
        self.get("/system/health").await
    }
    pub async fn tasks(&self) -> ClientResult<Vec<ScheduledTaskItem>> {
        self.get("/system/tasks").await
    }
    pub async fn task_runs(&self) -> ClientResult<Vec<TaskRunItem>> {
        self.get("/system/tasks/runs").await
    }
    pub async fn run_task(&self, name: &str) -> ClientResult<()> {
        self.send::<serde_json::Value>(
            self.request(reqwest::Method::POST, &format!("/system/tasks/{name}/run"))
                .json(&Empty {}),
        )
        .await
        .map(|_| ())
    }
    pub async fn backups(&self) -> ClientResult<Vec<BackupItem>> {
        self.get("/system/backups").await
    }
    pub async fn create_backup(&self) -> ClientResult<BackupItem> {
        self.post("/system/backups", &Empty {}).await
    }

    // --- library ------------------------------------------------------------------------------
    pub async fn library_stats(&self) -> ClientResult<LibraryStats> {
        self.get("/library/stats").await
    }
    pub async fn series(&self, q: &LibraryQuery) -> ClientResult<Page<SeriesListItem>> {
        self.get_query("/series", q).await
    }
    pub async fn series_detail(&self, id: i64) -> ClientResult<SeriesDetail> {
        self.get(&format!("/series/{id}")).await
    }
    pub async fn update_series(&self, id: i64, u: &SeriesUpdate) -> ClientResult<SeriesDetail> {
        self.patch(&format!("/series/{id}"), u).await
    }
    pub async fn delete_series(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/series/{id}")).await
    }
    pub async fn bulk_edit_series(&self, edit: &BulkEdit) -> ClientResult<BulkEditResult> {
        self.post("/series/bulk", edit).await
    }
    pub async fn episodes(&self, series_id: i64) -> ClientResult<Vec<EpisodeItem>> {
        self.get(&format!("/series/{series_id}/episodes")).await
    }
    pub async fn set_episodes_monitored(
        &self,
        body: &EpisodesMonitoredUpdate,
    ) -> ClientResult<BulkEditResult> {
        self.put("/episodes/monitor", body).await
    }
    pub async fn movies(&self, q: &LibraryQuery) -> ClientResult<Page<MovieListItem>> {
        self.get_query("/movies", q).await
    }
    pub async fn movie_detail(&self, id: i64) -> ClientResult<MovieDetail> {
        self.get(&format!("/movies/{id}")).await
    }
    pub async fn set_movie_monitored(&self, id: i64, monitored: bool) -> ClientResult<MovieDetail> {
        self.patch(
            &format!("/movies/{id}"),
            &serde_json::json!({ "monitored": monitored }),
        )
        .await
    }
    pub async fn delete_movie(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/movies/{id}")).await
    }
    pub async fn bulk_edit_movies(&self, edit: &BulkEdit) -> ClientResult<BulkEditResult> {
        self.post("/movies/bulk", edit).await
    }
    pub async fn refresh_series(&self, id: i64) -> ClientResult<RefreshResult> {
        self.post(&format!("/series/{id}/refresh"), &Empty {}).await
    }
    pub async fn refresh_movie(&self, id: i64) -> ClientResult<()> {
        self.post(&format!("/movies/{id}/refresh"), &Empty {}).await
    }

    // --- indexers and release search ---------------------------------------------------------
    pub async fn test_indexer(&self, req: &IndexerTestRequest) -> ClientResult<IndexerTestResult> {
        self.post("/providers/indexers/test", req).await
    }
    pub async fn queue(&self) -> ClientResult<kitsunarr_domain::api::queue::QueueResult> {
        self.get("/queue").await
    }
    pub async fn refresh_queue(&self) -> ClientResult<String> {
        self.post("/queue/refresh", &serde_json::json!({})).await
    }
    pub async fn grab_release(&self, req: &GrabRequest) -> ClientResult<GrabResult> {
        self.post("/releases/grab", req).await
    }
    // --- wanted ------------------------------------------------------------------------------
    pub async fn wanted_missing(
        &self,
        q: &kitsunarr_domain::api::wanted::WantedQuery,
    ) -> ClientResult<Page<kitsunarr_domain::api::wanted::WantedEpisode>> {
        self.get_query("/wanted/missing", q).await
    }
    pub async fn wanted_cutoff(
        &self,
        q: &kitsunarr_domain::api::wanted::WantedQuery,
    ) -> ClientResult<Page<kitsunarr_domain::api::wanted::WantedEpisode>> {
        self.get_query("/wanted/cutoff", q).await
    }
    /// Starts a background search for wanted episodes.
    pub async fn search_wanted(
        &self,
        req: &kitsunarr_domain::api::wanted::WantedSearchRequest,
    ) -> ClientResult<kitsunarr_domain::api::wanted::WantedSearchStarted> {
        self.post("/wanted/search", req).await
    }
    /// Automatic search: searches and grabs the best approved releases.
    pub async fn automatic_search(
        &self,
        q: &ReleaseQuery,
    ) -> ClientResult<kitsunarr_domain::api::search::AutomaticSearchResult> {
        self.post("/releases/search", q).await
    }
    pub async fn test_download_client(
        &self,
        req: &IndexerTestRequest,
    ) -> ClientResult<DownloadClientTestResult> {
        self.post("/providers/downloadclients/test", req).await
    }
    pub async fn search_releases(&self, q: &ReleaseQuery) -> ClientResult<ReleaseSearchResult> {
        self.get_query("/releases", q).await
    }

    // --- metadata catalogues --------------------------------------------------------------------
    pub async fn metadata_providers(&self) -> ClientResult<Vec<CatalogueInfo>> {
        self.get("/metadata/providers").await
    }
    pub async fn metadata_search(&self, q: &SearchQuery) -> ClientResult<Vec<SearchItem>> {
        self.get_query("/metadata/search", q).await
    }
    pub async fn folder_preview(
        &self,
        q: &kitsunarr_domain::api::metadata::FolderPreviewQuery,
    ) -> ClientResult<String> {
        self.get_query("/library/folder-preview", q).await
    }
    pub async fn add_title(&self, req: &AddTitleRequest) -> ClientResult<LibraryRef> {
        self.post("/library/add", req).await
    }
    pub async fn collections(&self) -> ClientResult<Vec<CollectionItem>> {
        self.get("/collections").await
    }
    pub async fn calendar(
        &self,
        start: &str,
        end: &str,
        unmonitored: bool,
    ) -> ClientResult<Vec<CalendarEntry>> {
        self.get_query(
            "/calendar",
            &[
                ("start", start),
                ("end", end),
                ("unmonitored", if unmonitored { "true" } else { "false" }),
            ],
        )
        .await
    }
    pub async fn history(
        &self,
        page: u32,
        page_size: u32,
        event_type: Option<&str>,
    ) -> ClientResult<Page<HistoryItem>> {
        let mut q = vec![
            ("page", page.to_string()),
            ("pageSize", page_size.to_string()),
        ];
        if let Some(e) = event_type {
            q.push(("eventType", e.to_string()));
        }
        self.get_query("/history", &q).await
    }

    // --- settings -----------------------------------------------------------------------------
    pub async fn tags(&self) -> ClientResult<Vec<TagItem>> {
        self.get("/tags").await
    }
    pub async fn create_tag(&self, label: &str) -> ClientResult<TagItem> {
        self.post(
            "/tags",
            &TagCreate {
                label: label.into(),
            },
        )
        .await
    }
    pub async fn delete_tag(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/tags/{id}")).await
    }
    pub async fn root_folders(&self) -> ClientResult<Vec<RootFolderItem>> {
        self.get("/rootfolders").await
    }
    pub async fn create_root_folder(&self, req: &RootFolderCreate) -> ClientResult<RootFolderItem> {
        self.post("/rootfolders", req).await
    }
    pub async fn delete_root_folder(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/rootfolders/{id}")).await
    }
    pub async fn quality_definitions(&self) -> ClientResult<Vec<QualityDefinition>> {
        self.get("/qualitydefinitions").await
    }
    pub async fn quality_profiles(&self) -> ClientResult<Vec<QualityProfile>> {
        self.get("/qualityprofiles").await
    }
    pub async fn update_quality_profile(&self, p: &QualityProfile) -> ClientResult<QualityProfile> {
        self.put(&format!("/qualityprofiles/{}", p.id), p).await
    }
    pub async fn create_quality_profile(&self, p: &QualityProfile) -> ClientResult<QualityProfile> {
        self.post("/qualityprofiles", p).await
    }
    pub async fn delete_quality_profile(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/qualityprofiles/{id}")).await
    }
    pub async fn custom_formats(&self) -> ClientResult<Vec<CustomFormat>> {
        self.get("/customformats").await
    }
    pub async fn delete_custom_format(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/customformats/{id}")).await
    }
    /// `kind` is `indexers`, `downloadclients`, `notifications` or `importlists`.
    pub async fn providers(&self, kind: &str) -> ClientResult<Vec<ProviderItem>> {
        self.get(&format!("/providers/{kind}")).await
    }
    pub async fn update_provider(
        &self,
        kind: &str,
        item: &ProviderItem,
    ) -> ClientResult<ProviderItem> {
        let body = serde_json::json!({
            "name": item.name, "implementation": item.implementation, "enabled": item.enabled,
            "priority": item.priority, "tags": item.tags, "options": item.options, "settings": item.settings,
        });
        self.put(&format!("/providers/{kind}/{}", item.id), &body)
            .await
    }
    pub async fn delete_provider(&self, kind: &str, id: i64) -> ClientResult<()> {
        self.delete(&format!("/providers/{kind}/{id}")).await
    }
    pub async fn naming(&self) -> ClientResult<NamingSettings> {
        self.get("/settings/naming").await
    }
    pub async fn put_naming(&self, s: &NamingSettings) -> ClientResult<NamingSettings> {
        self.put("/settings/naming", s).await
    }
    pub async fn media_management(&self) -> ClientResult<MediaManagementSettings> {
        self.get("/settings/mediamanagement").await
    }
    pub async fn put_media_management(
        &self,
        s: &MediaManagementSettings,
    ) -> ClientResult<MediaManagementSettings> {
        self.put("/settings/mediamanagement", s).await
    }
    pub async fn indexer_options(&self) -> ClientResult<IndexerOptionsSettings> {
        self.get("/settings/indexeroptions").await
    }
    pub async fn put_indexer_options(
        &self,
        s: &IndexerOptionsSettings,
    ) -> ClientResult<IndexerOptionsSettings> {
        self.put("/settings/indexeroptions", s).await
    }
    pub async fn general(&self) -> ClientResult<GeneralSettings> {
        self.get("/settings/general").await
    }
    pub async fn put_general(&self, s: &GeneralSettings) -> ClientResult<GeneralSettings> {
        self.put("/settings/general", s).await
    }

    // --- migration ----------------------------------------------------------------------------
    pub async fn migration_from_api(
        &self,
        req: &ApiSourceRequest,
    ) -> ClientResult<MigrationSession> {
        self.post("/migration/sessions", req).await
    }
    pub async fn migration_session(&self, id: &str) -> ClientResult<MigrationSession> {
        self.get(&format!("/migration/sessions/{id}")).await
    }
    pub async fn discard_migration(&self, id: &str) -> ClientResult<()> {
        self.delete(&format!("/migration/sessions/{id}")).await
    }
    pub async fn migration_plan(
        &self,
        id: &str,
        options: &MigrationOptions,
    ) -> ClientResult<MigrationPreview> {
        self.post(&format!("/migration/sessions/{id}/plan"), options)
            .await
    }
    pub async fn migration_apply(
        &self,
        id: &str,
        options: &MigrationOptions,
    ) -> ClientResult<MigrationSession> {
        self.post(&format!("/migration/sessions/{id}/apply"), options)
            .await
    }
    pub async fn migration_runs(&self) -> ClientResult<Vec<MigrationRunItem>> {
        self.get("/migration/runs").await
    }
    pub async fn migration_report(&self, id: i64) -> ClientResult<MigrationReport> {
        self.get(&format!("/migration/runs/{id}")).await
    }
    /// Native-only convenience: uploads a backup from memory.
    pub async fn migration_upload_bytes(&self, bytes: Vec<u8>) -> ClientResult<MigrationSession> {
        self.send(
            self.request(reqwest::Method::POST, "/migration/sessions/backup")
                .header("content-type", "application/octet-stream")
                .body(bytes),
        )
        .await
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn build_http() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
fn build_http() -> reqwest::Client {
    reqwest::Client::new()
}
