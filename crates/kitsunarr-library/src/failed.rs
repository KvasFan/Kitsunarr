//! Failed downloads: Sonarr's `FailedDownloadService`, with what its `DownloadFailedEvent`
//! handlers do: a "downloadFailed" history entry per episode (`HistoryService`), a blocklist
//! entry (`BlocklistService`), and the search to run again (`RedownloadFailedDownloadService`,
//! left to the caller, which owns searching).

use kitsunarr_db::blocklist::{self, NewBlocklist};
use kitsunarr_db::history::{self, GrabRecord, NewHistory};
use kitsunarr_db::{Database, DbResult};
use kitsunarr_domain::DownloadProtocol;

use crate::decision_input::data_number;

/// A search to run because a download failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redownload {
    /// 0 for a film's download.
    pub series_id: i64,
    /// A film's download (Tanukarr's movies): the film is searched for again.
    pub movie_id: Option<i64>,
    pub episode_ids: Vec<i64>,
    /// Sonarr's `ReleaseSourceType` of the failed grab (`InteractiveSearch`, `Search`, `Rss`).
    pub release_source: String,
}

/// Why and by what a download failed.
#[derive(Debug, Clone)]
pub struct Failure<'a> {
    pub message: &'a str,
    /// `Kitsunarr Failed Download Handling`, or none when marked by hand.
    pub source: Option<&'a str>,
    /// Torrents: the download id is the info hash.
    pub torrent_hash: Option<&'a str>,
    pub skip_redownload: bool,
}

/// `PublishDownloadFailedEvent` for a download's grabs (all with its download id): records the
/// failure and blocklists the release. Returns the search to run, unless skipped.
pub async fn mark_failed(
    db: &Database,
    grabs: &[GrabRecord],
    failure: &Failure<'_>,
) -> DbResult<Option<Redownload>> {
    // `GetGrabbedHistory` sorts newest first; the first one describes the release.
    let mut sorted: Vec<&GrabRecord> = grabs.iter().collect();
    sorted.sort_by(|a, b| b.date.cmp(&a.date));
    let Some(first) = sorted.first() else {
        return Ok(None);
    };
    let movie_id = first.movie_id;
    let Some(series_id) = first.series_id.or(movie_id.map(|_| 0)) else {
        return Ok(None);
    };
    let mut episode_ids: Vec<i64> = Vec::new();
    for g in &sorted {
        if let Some(e) = g.episode_id
            && !episode_ids.contains(&e)
        {
            episode_ids.push(e);
        }
    }
    let data: serde_json::Value = serde_json::from_str(&first.data).unwrap_or_default();
    let text = |k: &str| {
        data.get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    };
    let languages: Vec<i32> = first
        .languages
        .as_deref()
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_default();
    let revision: Option<serde_json::Value> = first
        .revision
        .as_deref()
        .and_then(|r| serde_json::from_str(r).ok());
    let protocol: DownloadProtocol = text("protocol")
        .and_then(|p| p.parse().ok())
        .unwrap_or(DownloadProtocol::Unknown);
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let release_source = text("releaseSource").unwrap_or_else(|| "Unknown".into());

    let mut tx = db.pool().begin().await?;
    if let Some(movie) = movie_id {
        history::insert(
            &mut tx,
            &NewHistory {
                event_type: "downloadFailed".into(),
                date: now.clone(),
                source_title: first.source_title.clone(),
                series_id: None,
                episode_id: None,
                movie_id: Some(movie),
                quality_id: first.quality_id.map(|q| q as i32),
                revision: revision.clone(),
                languages: languages.clone(),
                custom_format_score: None,
                download_id: Some(first.download_id.clone()),
                data: serde_json::json!({
                    "downloadClient": text("downloadClient"),
                    "downloadClientName": text("downloadClientName"),
                    "message": failure.message,
                    "source": failure.source,
                    "releaseSource": release_source,
                    "indexer": text("indexer"),
                    "size": text("size"),
                    "publishedDate": text("publishedDate"),
                }),
            },
        )
        .await?;
    }
    for episode_id in &episode_ids {
        history::insert(
            &mut tx,
            &NewHistory {
                event_type: "downloadFailed".into(),
                date: now.clone(),
                source_title: first.source_title.clone(),
                series_id: Some(series_id),
                episode_id: Some(*episode_id),
                movie_id: None,
                quality_id: first.quality_id.map(|q| q as i32),
                revision: revision.clone(),
                languages: languages.clone(),
                custom_format_score: None,
                download_id: Some(first.download_id.clone()),
                data: serde_json::json!({
                    "downloadClient": text("downloadClient"),
                    "downloadClientName": text("downloadClientName"),
                    "message": failure.message,
                    "source": failure.source,
                    "releaseSource": release_source,
                    "indexer": text("indexer"),
                    "size": text("size"),
                    "publishedDate": text("publishedDate"),
                }),
            },
        )
        .await?;
    }
    blocklist::insert(
        &mut *tx,
        &NewBlocklist {
            series_id: first.series_id,
            movie_id,
            episode_ids: episode_ids.clone(),
            source_title: first.source_title.clone(),
            quality_id: first.quality_id.map(|q| q as i32),
            revision,
            languages,
            date: now,
            published_date: text("publishedDate"),
            size: data_number(&data, "size"),
            protocol,
            indexer: text("indexer"),
            indexer_flags: data_number(&data, "indexerFlags").unwrap_or(0),
            release_type: text("releaseType"),
            message: Some(failure.message.to_string()),
            source: failure.source.map(str::to_string),
            torrent_info_hash: if protocol == DownloadProtocol::Torrent {
                failure
                    .torrent_hash
                    .map(str::to_string)
                    .or_else(|| text("torrentInfoHash"))
            } else {
                text("torrentInfoHash")
            },
        },
    )
    .await?;
    tx.commit().await?;
    tracing::info!(release = %first.source_title, message = failure.message, "download failed; release blocklisted");
    Ok((!failure.skip_redownload).then_some(Redownload {
        series_id,
        movie_id,
        episode_ids,
        release_source,
    }))
}
