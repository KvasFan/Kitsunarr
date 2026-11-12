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
    BulkEdit, BulkEditResult, CalendarEntry, CollectionDetail, CollectionEdit, CollectionItem,
    CollectionMemberEdit, EpisodeItem, EpisodesMonitoredUpdate, HistoryItem, LibraryQuery,
    LibraryStats, MovieDetail, MovieListItem, SeriesDetail, SeriesListItem, SeriesUpdate,
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
    CustomFormat, DownloadClientOptionsSettings, GeneralSettings, IndexerOptionsSettings,
    MediaManagementSettings, MetadataSourcesSettings, NamingSettings, ProviderItem, ProxySettings,
    QualityDefinition, QualityProfile, RootFolderCreate, RootFolderItem, TagCreate, TagItem,
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
    /// The orderings a catalogue title offers before it is added.
    pub async fn catalogue_orderings(
        &self,
        provider: kitsunarr_domain::MetadataProvider,
        id: &str,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library::EpisodeOrderingItem>> {
        self.get_query(
            "/metadata/orderings",
            &[("provider", provider.as_str()), ("id", id)],
        )
        .await
    }
    /// The lists of an import list's account (include/exclude pickers).
    pub async fn import_list_choices(
        &self,
        req: &kitsunarr_domain::api::search::IndexerTestRequest,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::ProviderChoice>> {
        self.post("/providers/importlists/lists", req).await
    }
    pub async fn series_orderings(
        &self,
        id: i64,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library::EpisodeOrderingItem>> {
        self.get(&format!("/series/{id}/orderings")).await
    }
    pub async fn set_series_ordering(
        &self,
        id: i64,
        ordering: Option<String>,
    ) -> ClientResult<RefreshResult> {
        self.put(
            &format!("/series/{id}/ordering"),
            &kitsunarr_domain::api::library::EpisodeOrderingUpdate { ordering },
        )
        .await
    }
    pub async fn delete_series(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/series/{id}")).await
    }
    /// Removes the series and its folder (to the recycle bin when one is set).
    pub async fn delete_series_with_files(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/series/{id}?deleteFiles=true")).await
    }
    pub async fn rescan_series(
        &self,
        id: i64,
    ) -> ClientResult<kitsunarr_domain::api::library::FileOperationResult> {
        self.post(&format!("/series/{id}/rescan"), &Empty {}).await
    }
    pub async fn rename_preview(
        &self,
        id: i64,
        season: Option<i32>,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library::RenamePreview>> {
        match season {
            Some(n) => {
                self.get(&format!("/series/{id}/rename?seasonNumber={n}"))
                    .await
            }
            None => self.get(&format!("/series/{id}/rename")).await,
        }
    }
    pub async fn rename_files(
        &self,
        id: i64,
        episode_file_ids: &[i64],
    ) -> ClientResult<kitsunarr_domain::api::library::FileOperationResult> {
        self.post(
            &format!("/series/{id}/rename"),
            &kitsunarr_domain::api::library::RenameRequest {
                episode_file_ids: episode_file_ids.to_vec(),
            },
        )
        .await
    }
    pub async fn delete_episode_file(&self, id: i64, file_id: i64) -> ClientResult<()> {
        self.delete(&format!("/series/{id}/files/{file_id}")).await
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
    /// `DELETE /queue/{downloadId}`: remove from the client and/or blocklist the release.
    pub async fn remove_queue_item(
        &self,
        download_id: &str,
        removal: &kitsunarr_domain::api::queue::QueueRemoval,
    ) -> ClientResult<kitsunarr_domain::api::queue::QueueRemovalResult> {
        let path = format!(
            "/queue/{}?removeFromClient={}&blocklist={}&skipRedownload={}",
            urlencoding(download_id),
            removal.remove_from_client,
            removal.blocklist,
            removal.skip_redownload
        );
        self.send(self.request(reqwest::Method::DELETE, &path))
            .await
    }
    pub async fn blocklist(
        &self,
        page: u32,
        page_size: u32,
    ) -> ClientResult<Page<kitsunarr_domain::api::queue::BlocklistItem>> {
        self.get(&format!("/blocklist?page={page}&pageSize={page_size}"))
            .await
    }
    pub async fn delete_blocklist(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/blocklist/{id}")).await
    }
    pub async fn remove_blocklist(
        &self,
        removal: &kitsunarr_domain::api::queue::BlocklistRemoval,
    ) -> ClientResult<u64> {
        self.post("/blocklist/remove", removal).await
    }
    pub async fn mark_history_failed(
        &self,
        id: i64,
    ) -> ClientResult<kitsunarr_domain::api::queue::QueueRemovalResult> {
        self.post(&format!("/history/{id}/failed"), &serde_json::json!({}))
            .await
    }
    pub async fn metadata_sources(&self) -> ClientResult<MetadataSourcesSettings> {
        self.get("/settings/metadatasources").await
    }
    pub async fn put_metadata_sources(
        &self,
        s: &MetadataSourcesSettings,
    ) -> ClientResult<MetadataSourcesSettings> {
        self.put("/settings/metadatasources", s).await
    }
    pub async fn download_client_options(&self) -> ClientResult<DownloadClientOptionsSettings> {
        self.get("/settings/downloadclientoptions").await
    }
    pub async fn put_download_client_options(
        &self,
        s: &DownloadClientOptionsSettings,
    ) -> ClientResult<DownloadClientOptionsSettings> {
        self.put("/settings/downloadclientoptions", s).await
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
    pub async fn test_import_list(
        &self,
        req: &IndexerTestRequest,
    ) -> ClientResult<DownloadClientTestResult> {
        self.post("/providers/importlists/test", req).await
    }
    // --- import lists ------------------------------------------------------------------------
    pub async fn sync_import_lists(
        &self,
        import_list_id: Option<i64>,
    ) -> ClientResult<kitsunarr_domain::api::import_lists::ImportListSyncResult> {
        self.post(
            "/importlists/sync",
            &kitsunarr_domain::api::import_lists::ImportListSyncRequest { import_list_id },
        )
        .await
    }
    pub async fn import_list_status(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::import_lists::ImportListStatusItem>> {
        self.get("/importlists/status").await
    }
    pub async fn import_list_entries(
        &self,
        import_list_id: i64,
    ) -> ClientResult<Vec<kitsunarr_domain::api::import_lists::ImportListEntry>> {
        self.get(&format!("/importlists/items?importListId={import_list_id}"))
            .await
    }
    pub async fn add_import_list_items(
        &self,
        import_list_id: i64,
        req: &kitsunarr_domain::api::import_lists::ImportListAddRequest,
    ) -> ClientResult<Vec<kitsunarr_domain::api::import_lists::ImportListAddOutcome>> {
        self.post(&format!("/importlists/{import_list_id}/add"), req)
            .await
    }
    pub async fn import_list_exclusions(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::import_lists::ImportListExclusionItem>> {
        self.get("/importlists/exclusions").await
    }
    pub async fn create_import_list_exclusion(
        &self,
        e: &kitsunarr_domain::api::import_lists::ImportListExclusionItem,
    ) -> ClientResult<kitsunarr_domain::api::import_lists::ImportListExclusionItem> {
        self.post("/importlists/exclusions", e).await
    }
    pub async fn delete_import_list_exclusion(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/importlists/exclusions/{id}")).await
    }
    pub async fn import_list_options(
        &self,
    ) -> ClientResult<kitsunarr_domain::api::settings::ImportListOptionsSettings> {
        self.get("/settings/importlistoptions").await
    }
    pub async fn put_import_list_options(
        &self,
        s: &kitsunarr_domain::api::settings::ImportListOptionsSettings,
    ) -> ClientResult<kitsunarr_domain::api::settings::ImportListOptionsSettings> {
        self.put("/settings/importlistoptions", s).await
    }
    pub async fn library_tools(
        &self,
    ) -> ClientResult<kitsunarr_domain::api::settings::LibraryToolsSettings> {
        self.get("/settings/librarytools").await
    }
    pub async fn put_library_tools(
        &self,
        s: &kitsunarr_domain::api::settings::LibraryToolsSettings,
    ) -> ClientResult<kitsunarr_domain::api::settings::LibraryToolsSettings> {
        self.put("/settings/librarytools", s).await
    }
    pub async fn storage_outlook(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library_tools::StorageOutlook>> {
        self.get("/librarytools/storage").await
    }
    pub async fn check_library(
        &self,
    ) -> ClientResult<kitsunarr_domain::api::library_tools::LibraryCheck> {
        self.post("/librarytools/audit", &serde_json::json!({}))
            .await
    }
    pub async fn recycle_bin(
        &self,
    ) -> ClientResult<kitsunarr_domain::api::library_tools::RecycleBin> {
        self.get("/librarytools/recycle").await
    }
    pub async fn restore_recycled(
        &self,
        req: &kitsunarr_domain::api::library_tools::RecycleRestoreRequest,
    ) -> ClientResult<String> {
        self.post("/librarytools/recycle/restore", req).await
    }
    pub async fn series_naming(
        &self,
        id: i64,
    ) -> ClientResult<kitsunarr_domain::api::library_tools::SeriesNamingOptions> {
        self.get(&format!("/series/{id}/naming")).await
    }
    pub async fn put_series_naming(
        &self,
        id: i64,
        o: &kitsunarr_domain::api::library_tools::SeriesNamingOptions,
    ) -> ClientResult<kitsunarr_domain::api::library_tools::SeriesNamingOptions> {
        self.put(&format!("/series/{id}/naming"), o).await
    }
    pub async fn companion_keys(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library_tools::CompanionKeyItem>> {
        self.get("/librarytools/keys").await
    }
    pub async fn create_companion_key(
        &self,
        req: &kitsunarr_domain::api::library_tools::CompanionKeyCreate,
    ) -> ClientResult<kitsunarr_domain::api::library_tools::CompanionKeyCreated> {
        self.post("/librarytools/keys", req).await
    }
    pub async fn delete_companion_key(&self, id: &str) -> ClientResult<()> {
        self.delete(&format!("/librarytools/keys/{id}")).await
    }
    pub async fn series_versions(
        &self,
        id: i64,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library_tools::RetainedVersion>> {
        self.get(&format!("/series/{id}/versions")).await
    }
    pub async fn activate_version(&self, id: i64, version: i64) -> ClientResult<String> {
        self.post(
            &format!("/series/{id}/versions/{version}/activate"),
            &serde_json::json!({}),
        )
        .await
    }
    /// Tanukarr's per-show compression: `{available, job, modes, message}` from the worker.
    pub async fn compression(&self, id: i64) -> ClientResult<serde_json::Value> {
        self.get(&format!("/series/{id}/compression")).await
    }
    pub async fn start_compression(
        &self,
        id: i64,
        mode: &str,
        min_size_mb: i64,
    ) -> ClientResult<serde_json::Value> {
        self.post(
            &format!("/series/{id}/compression"),
            &serde_json::json!({"mode": mode, "minSizeMb": min_size_mb}),
        )
        .await
    }
    pub async fn cancel_compression(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/series/{id}/compression")).await
    }
    pub async fn delete_version(&self, id: i64, version: i64) -> ClientResult<()> {
        self.delete(&format!("/series/{id}/versions/{version}"))
            .await
    }
    pub async fn manual_import_scan(
        &self,
        folder: &str,
        series_id: Option<i64>,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library_tools::ManualImportItem>> {
        let mut path = format!("/manualimport?folder={}", urlencoding(folder));
        if let Some(id) = series_id {
            path.push_str(&format!("&seriesId={id}"));
        }
        self.get(&path).await
    }
    pub async fn manual_import(
        &self,
        req: &kitsunarr_domain::api::library_tools::ManualImportRequest,
    ) -> ClientResult<Vec<kitsunarr_domain::api::library_tools::ManualImportResult>> {
        self.post("/manualimport", req).await
    }
    pub async fn delay_profiles(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::DelayProfile>> {
        self.get("/delayprofiles").await
    }
    pub async fn save_delay_profile(
        &self,
        d: &kitsunarr_domain::api::settings::DelayProfile,
    ) -> ClientResult<kitsunarr_domain::api::settings::DelayProfile> {
        if d.id > 0 {
            self.put(&format!("/delayprofiles/{}", d.id), d).await
        } else {
            self.post("/delayprofiles", d).await
        }
    }
    pub async fn delete_delay_profile(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/delayprofiles/{id}")).await
    }
    pub async fn release_profiles(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::ReleaseProfile>> {
        self.get("/releaseprofiles").await
    }
    pub async fn save_release_profile(
        &self,
        r: &kitsunarr_domain::api::settings::ReleaseProfile,
    ) -> ClientResult<kitsunarr_domain::api::settings::ReleaseProfile> {
        if r.id > 0 {
            self.put(&format!("/releaseprofiles/{}", r.id), r).await
        } else {
            self.post("/releaseprofiles", r).await
        }
    }
    pub async fn delete_release_profile(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/releaseprofiles/{id}")).await
    }
    pub async fn remote_path_mappings(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::RemotePathMapping>> {
        self.get("/remotepathmappings").await
    }
    pub async fn save_remote_path_mapping(
        &self,
        m: &kitsunarr_domain::api::settings::RemotePathMapping,
    ) -> ClientResult<kitsunarr_domain::api::settings::RemotePathMapping> {
        if m.id > 0 {
            self.put(&format!("/remotepathmappings/{}", m.id), m).await
        } else {
            self.post("/remotepathmappings", m).await
        }
    }
    pub async fn delete_remote_path_mapping(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/remotepathmappings/{id}")).await
    }
    pub async fn recent_searches(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::SearchActivity>> {
        self.get("/librarytools/searches").await
    }
    pub async fn notification_choices(
        &self,
        kind: &str,
        req: &IndexerTestRequest,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::ProviderChoice>> {
        self.post(&format!("/providers/notifications/choices/{kind}"), req)
            .await
    }
    pub async fn test_notification(
        &self,
        req: &IndexerTestRequest,
    ) -> ClientResult<DownloadClientTestResult> {
        self.post("/providers/notifications/test", req).await
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
    /// Monitored films without a file that are available.
    pub async fn missing_movies(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::wanted::WantedMovie>> {
        self.get("/wanted/movies").await
    }
    /// Searches for missing films (empty: all of them).
    pub async fn search_missing_movies(
        &self,
        movie_ids: &[i64],
    ) -> ClientResult<kitsunarr_domain::api::wanted::WantedSearchStarted> {
        self.post(
            "/wanted/movies/search",
            &kitsunarr_domain::api::wanted::WantedMovieSearchRequest {
                movie_ids: movie_ids.to_vec(),
            },
        )
        .await
    }
    /// Interactive release search for a film.
    pub async fn search_movie_releases(&self, movie_id: i64) -> ClientResult<ReleaseSearchResult> {
        self.get(&format!("/movies/{movie_id}/releases")).await
    }
    /// Automatic search for a film (grabs the best approved release).
    pub async fn automatic_movie_search(
        &self,
        movie_id: i64,
    ) -> ClientResult<kitsunarr_domain::api::search::AutomaticSearchResult> {
        self.post(
            &format!("/movies/{movie_id}/search"),
            &serde_json::json!({}),
        )
        .await
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
    pub async fn collection(&self, id: i64) -> ClientResult<CollectionDetail> {
        self.get(&format!("/collections/{id}")).await
    }
    pub async fn create_collection(&self, edit: &CollectionEdit) -> ClientResult<CollectionDetail> {
        self.post("/collections", edit).await
    }
    pub async fn update_collection(
        &self,
        id: i64,
        edit: &CollectionEdit,
    ) -> ClientResult<CollectionDetail> {
        self.put(&format!("/collections/{id}"), edit).await
    }
    pub async fn search_collection(
        &self,
        id: i64,
    ) -> ClientResult<kitsunarr_domain::api::wanted::WantedSearchStarted> {
        self.post(&format!("/collections/{id}/search"), &serde_json::json!({}))
            .await
    }
    pub async fn delete_collection(&self, id: i64) -> ClientResult<()> {
        self.delete(&format!("/collections/{id}")).await
    }
    pub async fn add_collection_member(
        &self,
        id: i64,
        edit: &CollectionMemberEdit,
    ) -> ClientResult<CollectionDetail> {
        self.post(&format!("/collections/{id}/members"), edit).await
    }
    pub async fn update_collection_member(
        &self,
        id: i64,
        member_id: i64,
        edit: &CollectionMemberEdit,
    ) -> ClientResult<CollectionDetail> {
        self.put(&format!("/collections/{id}/members/{member_id}"), edit)
            .await
    }
    pub async fn remove_collection_member(
        &self,
        id: i64,
        member_id: i64,
    ) -> ClientResult<CollectionDetail> {
        self.send(self.request(
            reqwest::Method::DELETE,
            &format!("/collections/{id}/members/{member_id}"),
        ))
        .await
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
    pub async fn update_quality_definitions(
        &self,
        defs: &[QualityDefinition],
    ) -> ClientResult<Vec<QualityDefinition>> {
        self.put("/qualitydefinitions", defs).await
    }
    pub async fn create_custom_quality(
        &self,
        c: &kitsunarr_domain::api::settings::CustomQualityCreate,
    ) -> ClientResult<QualityDefinition> {
        self.post("/qualitydefinitions", c).await
    }
    pub async fn codec_factors(
        &self,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::CodecFactorSummary>> {
        self.get("/qualitydefinitions/codecfactors").await
    }
    pub async fn update_codec_factors(
        &self,
        factors: &std::collections::BTreeMap<
            String,
            Option<kitsunarr_domain::api::settings::OrderedF64>,
        >,
    ) -> ClientResult<Vec<kitsunarr_domain::api::settings::CodecFactorSummary>> {
        self.put("/qualitydefinitions/codecfactors", factors).await
    }
    pub async fn retire_quality(&self, id: i32) -> ClientResult<()> {
        self.delete(&format!("/qualitydefinitions/{id}")).await
    }
    pub async fn restore_quality(&self, id: i32) -> ClientResult<()> {
        self.post::<serde_json::Value, _>(
            &format!("/qualitydefinitions/{id}/restore"),
            &serde_json::json!({}),
        )
        .await
        .map(|_| ())
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
    /// Blank providers of each implementation Kitsunarr runs (`GET /providers/{kind}/schema`).
    pub async fn provider_schema(&self, kind: &str) -> ClientResult<Vec<ProviderItem>> {
        self.get(&format!("/providers/{kind}/schema")).await
    }
    pub async fn create_provider(
        &self,
        kind: &str,
        item: &ProviderItem,
    ) -> ClientResult<ProviderItem> {
        let body = serde_json::json!({
            "name": item.name, "implementation": item.implementation, "enabled": item.enabled,
            "priority": item.priority, "tags": item.tags, "options": item.options, "settings": item.settings,
        });
        self.post(&format!("/providers/{kind}"), &body).await
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
    pub async fn proxy(&self) -> ClientResult<ProxySettings> {
        self.get("/settings/proxy").await
    }
    pub async fn put_proxy(&self, s: &ProxySettings) -> ClientResult<ProxySettings> {
        self.put("/settings/proxy", s).await
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

/// Percent-encodes a path segment (download ids may contain characters such as `|`).
fn urlencoding(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
