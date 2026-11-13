//! Indexers, download clients, notifications and import lists.
//!
//! Each provider keeps its implementation-specific `settings` verbatim (JSON). Kind-specific
//! switches live in typed columns (indexers, download clients) or in a JSON object
//! (`triggers` for notifications, `options` for import lists). The API layer converts records
//! to [`ProviderItem`] and redacts secrets.

use kitsunarr_domain::DownloadProtocol;
use kitsunarr_domain::api::settings::{ProviderItem, ProviderKind};
use serde_json::{Map, Value};
use sqlx::{FromRow, SqliteExecutor};

use crate::{DbError, DbResult, from_json, now, to_json};

/// Implementations Kitsunarr runs natively. Others are stored and shown as unsupported.
pub fn is_supported(kind: ProviderKind, implementation: &str) -> bool {
    let list: &[&str] = match kind {
        ProviderKind::Indexer => &[
            "Torznab",
            "Newznab",
            "Nyaa",
            "TorrentRssIndexer",
            "IPTorrents",
            "Torrentleech",
            "Fanzub",
            "FileList",
            "HDBits",
            "BroadcastheNet",
        ],
        ProviderKind::DownloadClient => {
            kitsunarr_domain::api::settings::DOWNLOAD_CLIENT_IMPLEMENTATIONS
        }
        // Kept in step with `kitsunarr_notifications::SUPPORTED`.
        ProviderKind::Notification => &[
            "Webhook",
            "Notifiarr",
            "Discord",
            "Slack",
            "Telegram",
            "Gotify",
            "Ntfy",
            "Pushover",
            "PushBullet",
            "Join",
            "Prowl",
            "Simplepush",
            "Signal",
            "Apprise",
            "MediaBrowser",
            "Jellyfin",
            "PlexServer",
            "Xbmc",
            "CustomScript",
            "Email",
            "Mailgun",
            "SendGrid",
        ],
        // Kept in step with `kitsunarr_importlists::SUPPORTED`.
        ProviderKind::ImportList => &[
            "AniListImport",
            "CustomImport",
            "SonarrImport",
            "PlexRssImport",
            "MDBListImport",
            "MyAnimeListImport",
            "TmdbListImport",
            "TraktListImport",
            "TraktUserImport",
            "TraktPopularImport",
            "TraktSmartListImport",
            "TmdbDiscoverImport",
            "TmdbAccountImport",
            "TmdbPersonImport",
            "SimklUserImport",
            "PlexImport",
        ],
    };
    list.iter().any(|i| i.eq_ignore_ascii_case(implementation))
}

pub fn protocol_for(implementation: &str) -> DownloadProtocol {
    match implementation.to_ascii_lowercase().as_str() {
        "torznab"
        | "qbittorrent"
        | "transmission"
        | "deluge"
        | "rtorrent"
        | "utorrent"
        | "vuze"
        | "aria2"
        | "flood"
        | "hadouken"
        | "downloadstation"
        | "torrentblackhole"
        | "freeboxdownload"
        | "rqbit"
        | "tribler"
        | "triblerdownloadclient"
        | "torrentdownloadstation"
        | "nyaa"
        | "torrentrssindexer"
        | "iptorrents"
        | "broadcasthenet"
        | "broadcastthenet"
        | "torrentleech"
        | "filelist"
        | "hdbits"
        | "torrentday" => DownloadProtocol::Torrent,
        "newznab"
        | "sabnzbd"
        | "nzbget"
        | "nzbvortex"
        | "pneumatic"
        | "usenetblackhole"
        | "usenetdownloadstation"
        | "omgwtfnzbs"
        | "fanzub" => DownloadProtocol::Usenet,
        _ => DownloadProtocol::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRecord {
    pub id: i64,
    pub kind: ProviderKind,
    pub name: String,
    pub implementation: String,
    pub protocol: DownloadProtocol,
    pub enabled: bool,
    pub priority: i32,
    pub tags: Vec<i64>,
    pub options: Map<String, Value>,
    pub settings: Map<String, Value>,
}

impl ProviderRecord {
    pub fn to_item(&self) -> ProviderItem {
        ProviderItem {
            id: self.id,
            kind: self.kind,
            name: self.name.clone(),
            implementation: self.implementation.clone(),
            protocol: match self.kind {
                ProviderKind::Indexer | ProviderKind::DownloadClient => Some(self.protocol),
                _ => None,
            },
            enabled: self.enabled,
            priority: self.priority,
            tags: self.tags.clone(),
            supported: is_supported(self.kind, &self.implementation),
            options: self.options.clone(),
            settings: self.settings.clone(),
        }
    }
}

fn opt_bool(options: &Map<String, Value>, key: &str, default: bool) -> bool {
    options.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn opt_i64(options: &Map<String, Value>, key: &str, default: i64) -> i64 {
    options.get(key).and_then(Value::as_i64).unwrap_or(default)
}

#[derive(FromRow)]
struct IndexerRow {
    id: i64,
    name: String,
    implementation: String,
    protocol: String,
    settings: String,
    enable_rss: bool,
    enable_automatic_search: bool,
    enable_interactive_search: bool,
    priority: i64,
    download_client_id: Option<i64>,
    season_search_maximum_single_episode_age: i64,
    tags: String,
}

#[derive(FromRow)]
struct ClientRow {
    id: i64,
    name: String,
    implementation: String,
    protocol: String,
    settings: String,
    enabled: bool,
    priority: i64,
    remove_completed_downloads: bool,
    remove_failed_downloads: bool,
    tags: String,
}

#[derive(FromRow)]
struct NotificationRow {
    id: i64,
    name: String,
    implementation: String,
    settings: String,
    triggers: String,
    tags: String,
}

#[derive(FromRow)]
struct ImportListRow {
    id: i64,
    name: String,
    implementation: String,
    settings: String,
    options: String,
    enabled: bool,
    tags: String,
}

fn parse_protocol(s: &str) -> DownloadProtocol {
    s.parse().unwrap_or(DownloadProtocol::Unknown)
}

pub async fn list(
    ex: impl SqliteExecutor<'_>,
    kind: ProviderKind,
) -> DbResult<Vec<ProviderRecord>> {
    let records = match kind {
        ProviderKind::Indexer => {
            let rows: Vec<IndexerRow> =
                sqlx::query_as("SELECT * FROM indexers ORDER BY priority, name")
                    .fetch_all(ex)
                    .await?;
            rows.into_iter()
                .map(|r| {
                    let mut options = Map::new();
                    options.insert("enableRss".into(), r.enable_rss.into());
                    options.insert(
                        "enableAutomaticSearch".into(),
                        r.enable_automatic_search.into(),
                    );
                    options.insert(
                        "enableInteractiveSearch".into(),
                        r.enable_interactive_search.into(),
                    );
                    options.insert(
                        "downloadClientId".into(),
                        r.download_client_id.map_or(Value::Null, Value::from),
                    );
                    options.insert(
                        "seasonSearchMaximumSingleEpisodeAge".into(),
                        r.season_search_maximum_single_episode_age.into(),
                    );
                    Ok(ProviderRecord {
                        id: r.id,
                        kind,
                        name: r.name,
                        implementation: r.implementation,
                        protocol: parse_protocol(&r.protocol),
                        enabled: r.enable_rss
                            || r.enable_automatic_search
                            || r.enable_interactive_search,
                        priority: r.priority as i32,
                        tags: from_json(&r.tags)?,
                        options,
                        settings: from_json(&r.settings)?,
                    })
                })
                .collect::<DbResult<Vec<_>>>()?
        }
        ProviderKind::DownloadClient => {
            let rows: Vec<ClientRow> =
                sqlx::query_as("SELECT * FROM download_clients ORDER BY priority, name")
                    .fetch_all(ex)
                    .await?;
            rows.into_iter()
                .map(|r| {
                    let mut options = Map::new();
                    options.insert(
                        "removeCompletedDownloads".into(),
                        r.remove_completed_downloads.into(),
                    );
                    options.insert(
                        "removeFailedDownloads".into(),
                        r.remove_failed_downloads.into(),
                    );
                    Ok(ProviderRecord {
                        id: r.id,
                        kind,
                        name: r.name,
                        implementation: r.implementation,
                        protocol: parse_protocol(&r.protocol),
                        enabled: r.enabled,
                        priority: r.priority as i32,
                        tags: from_json(&r.tags)?,
                        options,
                        settings: from_json(&r.settings)?,
                    })
                })
                .collect::<DbResult<Vec<_>>>()?
        }
        ProviderKind::Notification => {
            let rows: Vec<NotificationRow> =
                sqlx::query_as("SELECT * FROM notifications ORDER BY name")
                    .fetch_all(ex)
                    .await?;
            rows.into_iter()
                .map(|r| {
                    let options: Map<String, Value> = from_json(&r.triggers)?;
                    let enabled = options.values().any(|v| v.as_bool() == Some(true));
                    Ok(ProviderRecord {
                        id: r.id,
                        kind,
                        name: r.name,
                        implementation: r.implementation,
                        protocol: DownloadProtocol::Unknown,
                        enabled,
                        priority: 0,
                        tags: from_json(&r.tags)?,
                        options,
                        settings: from_json(&r.settings)?,
                    })
                })
                .collect::<DbResult<Vec<_>>>()?
        }
        ProviderKind::ImportList => {
            let rows: Vec<ImportListRow> =
                sqlx::query_as("SELECT * FROM import_lists ORDER BY name")
                    .fetch_all(ex)
                    .await?;
            rows.into_iter()
                .map(|r| {
                    Ok(ProviderRecord {
                        id: r.id,
                        kind,
                        name: r.name,
                        implementation: r.implementation,
                        protocol: DownloadProtocol::Unknown,
                        enabled: r.enabled,
                        priority: 0,
                        tags: from_json(&r.tags)?,
                        options: from_json(&r.options)?,
                        settings: from_json(&r.settings)?,
                    })
                })
                .collect::<DbResult<Vec<_>>>()?
        }
    };
    Ok(records)
}

pub async fn get(
    ex: impl SqliteExecutor<'_>,
    kind: ProviderKind,
    id: i64,
) -> DbResult<ProviderRecord> {
    list(ex, kind)
        .await?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(DbError::NotFound)
}

fn validate(p: &ProviderRecord) -> DbResult<()> {
    if p.name.trim().is_empty() {
        return Err(DbError::Invalid("name is required".into()));
    }
    if p.implementation.trim().is_empty() {
        return Err(DbError::Invalid("implementation is required".into()));
    }
    Ok(())
}

/// Inserts a provider and returns its id.
pub async fn create(ex: impl SqliteExecutor<'_>, p: &ProviderRecord) -> DbResult<i64> {
    validate(p)?;
    let tags = to_json(&p.tags)?;
    let settings = to_json(&p.settings)?;
    let ts = now();
    let o = &p.options;
    let result = match p.kind {
        ProviderKind::Indexer => {
            sqlx::query_scalar(
                "INSERT INTO indexers (name, implementation, protocol, settings, enable_rss, enable_automatic_search, \
                 enable_interactive_search, priority, download_client_id, season_search_maximum_single_episode_age, tags, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(p.name.trim())
            .bind(&p.implementation)
            .bind(p.protocol.as_str())
            .bind(&settings)
            .bind(opt_bool(o, "enableRss", true))
            .bind(opt_bool(o, "enableAutomaticSearch", true))
            .bind(opt_bool(o, "enableInteractiveSearch", true))
            .bind(p.priority)
            .bind(o.get("downloadClientId").and_then(Value::as_i64).filter(|v| *v > 0))
            .bind(opt_i64(o, "seasonSearchMaximumSingleEpisodeAge", 0))
            .bind(&tags)
            .bind(&ts)
            .fetch_one(ex)
            .await
        }
        ProviderKind::DownloadClient => {
            sqlx::query_scalar(
                "INSERT INTO download_clients (name, implementation, protocol, settings, enabled, priority, \
                 remove_completed_downloads, remove_failed_downloads, tags, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(p.name.trim())
            .bind(&p.implementation)
            .bind(p.protocol.as_str())
            .bind(&settings)
            .bind(p.enabled)
            .bind(p.priority)
            .bind(opt_bool(o, "removeCompletedDownloads", true))
            .bind(opt_bool(o, "removeFailedDownloads", true))
            .bind(&tags)
            .bind(&ts)
            .fetch_one(ex)
            .await
        }
        ProviderKind::Notification => {
            sqlx::query_scalar(
                "INSERT INTO notifications (name, implementation, settings, triggers, tags, created_at) VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(p.name.trim())
            .bind(&p.implementation)
            .bind(&settings)
            .bind(to_json(o)?)
            .bind(&tags)
            .bind(&ts)
            .fetch_one(ex)
            .await
        }
        ProviderKind::ImportList => {
            sqlx::query_scalar(
                "INSERT INTO import_lists (name, implementation, settings, options, enabled, tags, created_at) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(p.name.trim())
            .bind(&p.implementation)
            .bind(&settings)
            .bind(to_json(o)?)
            .bind(p.enabled)
            .bind(&tags)
            .bind(&ts)
            .fetch_one(ex)
            .await
        }
    };
    result.map_err(|e| DbError::from_write(e, "provider"))
}

/// Replaces a provider's mutable fields.
pub async fn update(ex: impl SqliteExecutor<'_>, p: &ProviderRecord) -> DbResult<()> {
    validate(p)?;
    let tags = to_json(&p.tags)?;
    let settings = to_json(&p.settings)?;
    let o = &p.options;
    let r = match p.kind {
        ProviderKind::Indexer => sqlx::query(
            "UPDATE indexers SET name = ?, implementation = ?, protocol = ?, settings = ?, enable_rss = ?, \
             enable_automatic_search = ?, enable_interactive_search = ?, priority = ?, download_client_id = ?, \
             season_search_maximum_single_episode_age = ?, tags = ? WHERE id = ?",
        )
        .bind(p.name.trim())
        .bind(&p.implementation)
        .bind(p.protocol.as_str())
        .bind(&settings)
        .bind(opt_bool(o, "enableRss", true))
        .bind(opt_bool(o, "enableAutomaticSearch", true))
        .bind(opt_bool(o, "enableInteractiveSearch", true))
        .bind(p.priority)
        .bind(o.get("downloadClientId").and_then(Value::as_i64).filter(|v| *v > 0))
        .bind(opt_i64(o, "seasonSearchMaximumSingleEpisodeAge", 0))
        .bind(&tags)
        .bind(p.id)
        .execute(ex)
        .await,
        ProviderKind::DownloadClient => sqlx::query(
            "UPDATE download_clients SET name = ?, implementation = ?, protocol = ?, settings = ?, enabled = ?, \
             priority = ?, remove_completed_downloads = ?, remove_failed_downloads = ?, tags = ? WHERE id = ?",
        )
        .bind(p.name.trim())
        .bind(&p.implementation)
        .bind(p.protocol.as_str())
        .bind(&settings)
        .bind(p.enabled)
        .bind(p.priority)
        .bind(opt_bool(o, "removeCompletedDownloads", true))
        .bind(opt_bool(o, "removeFailedDownloads", true))
        .bind(&tags)
        .bind(p.id)
        .execute(ex)
        .await,
        ProviderKind::Notification => {
            sqlx::query("UPDATE notifications SET name = ?, implementation = ?, settings = ?, triggers = ?, tags = ? WHERE id = ?")
                .bind(p.name.trim())
                .bind(&p.implementation)
                .bind(&settings)
                .bind(to_json(o)?)
                .bind(&tags)
                .bind(p.id)
                .execute(ex)
                .await
        }
        ProviderKind::ImportList => sqlx::query(
            "UPDATE import_lists SET name = ?, implementation = ?, settings = ?, options = ?, enabled = ?, tags = ? WHERE id = ?",
        )
        .bind(p.name.trim())
        .bind(&p.implementation)
        .bind(&settings)
        .bind(to_json(o)?)
        .bind(p.enabled)
        .bind(&tags)
        .bind(p.id)
        .execute(ex)
        .await,
    }
    .map_err(|e| DbError::from_write(e, "provider"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn delete(ex: impl SqliteExecutor<'_>, kind: ProviderKind, id: i64) -> DbResult<()> {
    let sql = match kind {
        ProviderKind::Indexer => "DELETE FROM indexers WHERE id = ?",
        ProviderKind::DownloadClient => "DELETE FROM download_clients WHERE id = ?",
        ProviderKind::Notification => "DELETE FROM notifications WHERE id = ?",
        ProviderKind::ImportList => "DELETE FROM import_lists WHERE id = ?",
    };
    let r = sqlx::query(sql).bind(id).execute(ex).await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: ProviderKind, name: &str, implementation: &str) -> ProviderRecord {
        ProviderRecord {
            id: 0,
            kind,
            name: name.into(),
            implementation: implementation.into(),
            protocol: protocol_for(implementation),
            enabled: true,
            priority: 25,
            tags: vec![],
            options: Map::new(),
            settings: serde_json::json!({"baseUrl": "http://x", "apiKey": "secret"})
                .as_object()
                .unwrap()
                .clone(),
        }
    }

    #[tokio::test]
    async fn crud_for_every_kind() {
        let db = crate::test_support::db().await;
        for (kind, imp) in [
            (ProviderKind::Indexer, "Torznab"),
            (ProviderKind::Indexer, "Nyaa"),
            (ProviderKind::Indexer, "TorrentRssIndexer"),
            (ProviderKind::Indexer, "IPTorrents"),
            (ProviderKind::Indexer, "Torrentleech"),
            (ProviderKind::Indexer, "Fanzub"),
            (ProviderKind::Indexer, "FileList"),
            (ProviderKind::Indexer, "HDBits"),
            (ProviderKind::Indexer, "BroadcastheNet"),
            (ProviderKind::DownloadClient, "QBittorrent"),
            (ProviderKind::Notification, "Webhook"),
            (ProviderKind::ImportList, "SonarrImport"),
        ] {
            let mut r = record(kind, "one", imp);
            r.options.insert("onGrab".into(), true.into());
            // Templates send `downloadClientId: 0` for "any client": stored as none.
            r.options.insert("downloadClientId".into(), 0.into());
            let id = create(db.pool(), &r).await.unwrap();
            let mut got = get(db.pool(), kind, id).await.unwrap();
            assert_eq!(got.settings.get("apiKey").unwrap(), "secret");
            assert_eq!(got.to_item().supported, is_supported(kind, imp));
            got.name = "renamed".into();
            update(db.pool(), &got).await.unwrap();
            assert_eq!(list(db.pool(), kind).await.unwrap()[0].name, "renamed");
            assert!(matches!(
                create(db.pool(), &got).await,
                Err(DbError::Conflict(_))
            ));
            delete(db.pool(), kind, id).await.unwrap();
            assert!(list(db.pool(), kind).await.unwrap().is_empty());
        }
        assert_eq!(protocol_for("Sabnzbd"), DownloadProtocol::Usenet);
        assert_eq!(protocol_for("Fanzub"), DownloadProtocol::Usenet);
        for torrent in [
            "TorrentRssIndexer",
            "IPTorrents",
            "Torrentleech",
            "FileList",
            "HDBits",
            "BroadcastheNet",
        ] {
            assert_eq!(
                protocol_for(torrent),
                DownloadProtocol::Torrent,
                "{torrent}"
            );
            assert!(is_supported(ProviderKind::Indexer, torrent));
        }
    }
}
