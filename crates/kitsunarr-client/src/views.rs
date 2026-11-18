//! Endpoints of Tanukarr's interface pages: naming samples, statistics, collection settings,
//! host settings, log files and the parse tester.

use kitsunarr_domain::api::library::CollectionDetail;
use kitsunarr_domain::api::settings::NamingSettings;
use kitsunarr_domain::api::views::{
    CollectionSettingsUpdate, HostSettings, LibraryStatistics, LogFileItem, NamingExamples,
    ParseResult, StatisticsQuery,
};

use crate::{Client, ClientError, ClientResult};

impl Client {
    /// The names the (unsaved) naming settings give Sonarr's sample titles.
    pub async fn naming_examples(&self, s: &NamingSettings) -> ClientResult<NamingExamples> {
        self.post("/settings/naming/examples", s).await
    }

    pub async fn statistics(&self, q: &StatisticsQuery) -> ClientResult<LibraryStatistics> {
        self.get_query("/statistics", q).await
    }

    pub async fn update_collection_settings(
        &self,
        id: i64,
        u: &CollectionSettingsUpdate,
    ) -> ClientResult<CollectionDetail> {
        self.put(&format!("/collections/{id}/settings"), u).await
    }

    pub async fn host_settings(&self) -> ClientResult<HostSettings> {
        self.get("/settings/host").await
    }

    pub async fn put_host_settings(&self, s: &HostSettings) -> ClientResult<HostSettings> {
        self.put("/settings/host", s).await
    }

    pub async fn log_files(&self) -> ClientResult<Vec<LogFileItem>> {
        self.get("/system/logs/files").await
    }

    /// A log file's text.
    pub async fn log_file_text(&self, name: &str) -> ClientResult<String> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/system/logs/files/{}", crate::urlencoding(name)),
            )
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(ClientError::Api {
                status: status.as_u16(),
                body: serde_json::from_str(&text).unwrap_or_else(|_| {
                    kitsunarr_domain::api::error::ApiErrorBody {
                        code: "http_error".into(),
                        message: format!("The server answered {status}."),
                        fields: Vec::new(),
                    }
                }),
            });
        }
        Ok(text)
    }

    /// Where a log file downloads from (the browser sends its session cookie).
    pub fn log_file_download_url(&self, name: &str) -> String {
        self.url(&format!(
            "/system/logs/files/{}?download=true",
            crate::urlencoding(name)
        ))
    }

    pub async fn parse(&self, title: &str) -> ClientResult<ParseResult> {
        self.get_query("/parse", &[("title", title)]).await
    }
}
