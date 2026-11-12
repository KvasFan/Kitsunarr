//! Builds notification events from the library and sends them in the background
//! (Sonarr's `NotificationService` handlers).

use kitsunarr_db::library::series;
use kitsunarr_db::providers;
use kitsunarr_db::{Database, settings as kv, tags};
use kitsunarr_domain::api::settings::{GeneralSettings, ProviderKind};
use kitsunarr_domain::{ExternalIdKind, Language, Quality, SeriesType};
use kitsunarr_notifications::{
    Connection, EpisodeRef, Event, FileRef, ImageRef, Instance, LanguageRef, Notifier, SeriesRef,
};

/// The stored notification connections.
pub async fn connections(db: &Database) -> Vec<Connection> {
    providers::list(db.pool(), ProviderKind::Notification)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| Connection {
            id: p.id,
            name: p.name,
            implementation: p.implementation,
            triggers: p.options,
            tags: p.tags,
            settings: p.settings,
        })
        .collect()
}

pub async fn instance(db: &Database) -> Instance {
    let general: GeneralSettings = kv::get_or_default(db.pool(), kv::KEY_GENERAL)
        .await
        .unwrap_or_default();
    Instance {
        name: general.instance_name,
        application_url: None,
    }
}

pub fn language(id: i32) -> Option<LanguageRef> {
    Language::by_id(id).map(|l| LanguageRef {
        id: l.id,
        name: l.name.to_string(),
    })
}

pub fn quality_name(id: i64) -> String {
    Quality::by_id(id as i32)
        .map(|q| q.name.to_string())
        .unwrap_or_else(|| "Unknown".into())
}

/// `WebhookSeries` for a library series.
pub async fn series_ref(db: &Database, id: i64) -> Option<SeriesRef> {
    let d = series::get_detail(db, id).await.ok()?;
    let labels = tags::list(db.pool()).await.unwrap_or_default();
    let ext = |kind: ExternalIdKind| {
        d.external_ids
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.value.clone())
            .collect::<Vec<_>>()
    };
    let num = |kind| {
        ext(kind)
            .first()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let nums = |kind| {
        ext(kind)
            .iter()
            .filter_map(|v| v.parse::<i64>().ok())
            .collect::<Vec<_>>()
    };
    let original_language = series::original_language(db, id)
        .await
        .ok()
        .flatten()
        .and_then(|l| language(l as i32));
    Some(SeriesRef {
        id,
        title: d.item.title.clone(),
        title_slug: kitsunarr_domain::text::slugify(&d.item.title),
        path: d.item.path.clone(),
        tvdb_id: num(ExternalIdKind::Tvdb),
        tv_maze_id: num(ExternalIdKind::TvMaze),
        tmdb_id: num(ExternalIdKind::Tmdb),
        imdb_id: ext(ExternalIdKind::Imdb).first().cloned(),
        mal_ids: nums(ExternalIdKind::MyAnimeList),
        ani_list_ids: nums(ExternalIdKind::AniList),
        series_type: match d.item.series_type {
            SeriesType::Anime => "anime",
            SeriesType::Daily => "daily",
            _ => "standard",
        }
        .into(),
        year: d.item.year.unwrap_or(0),
        genres: d.genres.clone(),
        images: d
            .images
            .iter()
            .map(|i| ImageRef {
                cover_type: i.cover_type.clone(),
                url: None,
                remote_url: Some(i.remote_url.clone()),
            })
            .collect(),
        tags: labels
            .iter()
            .filter(|t| d.item.tags.contains(&t.id))
            .map(|t| t.label.clone())
            .collect(),
        tag_ids: d.item.tags.clone(),
        original_language,
        overview: d.overview.clone(),
        rating: None,
    })
}

/// `WebhookEpisode`s, in the order given.
pub async fn episode_refs(db: &Database, series_id: i64, ids: &[i64]) -> Vec<EpisodeRef> {
    let all = series::list_episodes(db, series_id, None)
        .await
        .unwrap_or_default();
    ids.iter()
        .filter_map(|id| all.iter().find(|e| e.id == *id))
        .map(|e| EpisodeRef {
            id: e.id,
            episode_number: e.episode_number,
            season_number: e.season_number,
            title: e.title.clone(),
            overview: None,
            air_date: e.air_date.clone(),
            air_date_utc: e.air_date_utc.clone(),
            series_id,
            tvdb_id: 0,
        })
        .collect()
}

/// `WebhookEpisodeFile` for a stored file.
pub async fn file_ref(db: &Database, series: &SeriesRef, file_id: i64) -> Option<FileRef> {
    let f = series::file_records(db, series.id)
        .await
        .ok()?
        .into_iter()
        .find(|f| f.id == file_id)?;
    Some(FileRef {
        id: f.id,
        path: format!("{}/{}", series.path.trim_end_matches('/'), f.relative_path),
        relative_path: f.relative_path.clone(),
        quality: quality_name(f.quality_id),
        quality_version: f.revision_version as i32,
        release_group: f.release_group.clone(),
        scene_name: f.scene_name.clone(),
        size: f.size,
        date_added: f.added_at.clone(),
        languages: f.language_ids().into_iter().filter_map(language).collect(),
        source_path: None,
        recycle_bin_path: None,
    })
}

/// Sends an event to the connections that want it, in the background.
pub fn spawn(db: &Database, notifier: &Notifier, event: Event) {
    let (db, notifier) = (db.clone(), notifier.clone());
    tokio::spawn(async move {
        let connections = connections(&db).await;
        if connections.is_empty() {
            return;
        }
        // Jellyfin connections with a selected library (Tanukarr's `Jellyfin`) only refresh that
        // library after imports, through the durable refresh queue.
        let (queued, others): (Vec<Connection>, Vec<Connection>) = connections
            .into_iter()
            .partition(crate::jellyfin_refresh::is_queue_managed);
        if matches!(event, Event::Download(_)) {
            for c in queued.iter().filter(|c| c.wants(&event)) {
                if let Some(r) = crate::jellyfin_refresh::RefreshSettings::from_connection(c) {
                    crate::jellyfin_refresh::on_import(&db, notifier.http(), &r).await;
                }
            }
        }
        let instance = instance(&db).await;
        notifier.dispatch(&others, &event, &instance).await;
    });
}

/// Builds and sends the "Download" (or upgrade) notification for an imported file.
#[allow(clippy::too_many_arguments)]
pub async fn imported(
    db: &Database,
    notifier: &Notifier,
    series_id: i64,
    r: &crate::import::FileResult,
    f: &crate::import::ImportedFile,
    download_id: &str,
    client_name: &str,
    client_implementation: &str,
) {
    let Some(series) = series_ref(db, series_id).await else {
        return;
    };
    let Some(mut file) = file_ref(db, &series, f.file_id).await else {
        return;
    };
    file.source_path = Some(f.source_path.clone());
    let episodes = episode_refs(db, series_id, &r.episode_ids).await;
    let grabs = kitsunarr_db::history::grabs_for(db, &[download_id.to_string()])
        .await
        .unwrap_or_default();
    let data: serde_json::Value = grabs
        .first()
        .and_then(|g| serde_json::from_str(&g.data).ok())
        .unwrap_or_default();
    let text = |k: &str| data.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let deleted_files = f
        .replaced
        .iter()
        .map(|rel| kitsunarr_notifications::FileRef {
            relative_path: rel.clone(),
            path: format!("{}/{rel}", series.path.trim_end_matches('/')),
            ..Default::default()
        })
        .collect();
    let event =
        kitsunarr_notifications::Event::Download(Box::new(kitsunarr_notifications::Download {
            series,
            episodes,
            file,
            release: kitsunarr_notifications::GrabbedRef {
                release_title: grabs.first().map(|g| g.source_title.clone()),
                indexer: text("indexer"),
                size: text("size").and_then(|v| v.parse().ok()),
                indexer_flags: Vec::new(),
                release_type: text("releaseType"),
            },
            deleted_files,
            download_client: Some(client_name.to_string()),
            download_client_type: Some(client_implementation.to_string()),
            download_id: Some(download_id.to_string()),
        }));
    spawn(db, notifier, event);
}
