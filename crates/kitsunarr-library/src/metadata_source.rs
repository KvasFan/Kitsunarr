//! Changing catalogue ownership with a reviewable preview and one SQLite transaction.
//! Episode identities belong to a catalogue: they must never be reused across catalogues.

use std::collections::{HashMap, HashSet};

use kitsunarr_db::{
    Database, DbError,
    library::{movies, series},
};
use kitsunarr_domain::api::metadata::{
    EntryKind, MetadataSourcePreview, MetadataSourceRequest, RefreshResult, SourceEpisodeMapping,
    SourceEpisodePreview, SourceTargetEpisode,
};
use kitsunarr_domain::{ExternalId, ExternalIdKind, MetadataProvider};
use kitsunarr_metadata::{Metadata, SeriesInfo};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use crate::{LibraryError, LibraryResult};

#[derive(Debug, Serialize, sqlx::FromRow)]
struct Owner {
    metadata_provider: String,
    metadata_id: String,
    monitored: bool,
    overview: Option<String>,
    images: String,
    context: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct Episode {
    id: i64,
    season_number: i32,
    episode_number: i32,
    title: Option<String>,
    air_date: Option<String>,
    monitored: bool,
    episode_file_id: Option<i64>,
    provider_episode_id: Option<String>,
    retained: bool,
}

fn identity(provider: MetadataProvider, id: &str) -> ExternalId {
    ExternalId {
        kind: match provider {
            MetadataProvider::Tvdb => ExternalIdKind::Tvdb,
            MetadataProvider::TmdbTv | MetadataProvider::TmdbMovie => ExternalIdKind::Tmdb,
            MetadataProvider::TvMaze => ExternalIdKind::TvMaze,
            MetadataProvider::AniList => ExternalIdKind::AniList,
            MetadataProvider::MyAnimeList => ExternalIdKind::MyAnimeList,
            MetadataProvider::Omdb => ExternalIdKind::Imdb,
        },
        value: id.to_string(),
    }
}

async fn owner(conn: &mut SqliteConnection, movie: bool, id: i64) -> LibraryResult<Owner> {
    Ok(sqlx::query_as(if movie {
        "SELECT metadata_provider, metadata_id, monitored, overview, images, json_object('path', path, 'profile', quality_profile_id, 'minimumAvailability', minimum_availability) AS context FROM movies WHERE id = ?"
    } else {
        "SELECT metadata_provider, metadata_id, monitored, overview, images, json_object('path', path, 'profile', quality_profile_id, 'ordering', episode_ordering, 'monitorNew', monitor_new_items, 'seasons', (SELECT json_group_array(json_array(season_number, monitored)) FROM seasons WHERE series_id = ?1)) AS context FROM series WHERE id = ?1"
    }).bind(id).fetch_optional(conn).await?.ok_or(DbError::NotFound)?)
}

async fn fetch(
    db: &Database,
    metadata: &Metadata,
    movie: bool,
    id: i64,
    req: &MetadataSourceRequest,
) -> LibraryResult<SeriesInfo> {
    let current = owner(&mut *db.pool().acquire().await?, movie, id).await?;
    let key = req.id.trim();
    if key.is_empty() || key.len() > 100 {
        return Err(LibraryError::invalid(
            "id",
            "Enter a catalogue ID of at most 100 characters.",
        ));
    }
    if current.metadata_provider == req.provider.as_str() && current.metadata_id == key {
        let retained: bool = if movie {
            false
        } else {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM retained_metadata_episodes r JOIN episodes e ON e.id = r.episode_id WHERE e.series_id = ?)").bind(id).fetch_one(db.pool()).await?
        };
        if !retained {
            return Err(LibraryError::invalid(
                "provider",
                "This title already uses that catalogue identity.",
            ));
        }
    }
    let language = if movie {
        None
    } else {
        Some(crate::series_language_code(db, id).await?)
    };
    let info = metadata
        .series_fresh_in(req.provider, key, None, language.as_deref())
        .await?;
    if (info.kind == EntryKind::Movie) != movie {
        return Err(LibraryError::invalid(
            "provider",
            "Choose a catalogue entry of the same media type.",
        ));
    }
    if info.provider != req.provider || info.id != key {
        return Err(LibraryError::invalid(
            "id",
            "The catalogue returned a different identity.",
        ));
    }
    Ok(info)
}

/// Reads the target catalogue without changing the library or files.
pub async fn preview(
    db: &Database,
    metadata: &Metadata,
    movie: bool,
    id: i64,
    req: &MetadataSourceRequest,
) -> LibraryResult<MetadataSourcePreview> {
    let info = fetch(db, metadata, movie, id, req).await?;
    let mut tx = db.pool().begin().await?;
    plan(&mut tx, movie, id, req, &info).await
}

async fn plan(
    conn: &mut SqliteConnection,
    movie: bool,
    id: i64,
    req: &MetadataSourceRequest,
    info: &SeriesInfo,
) -> LibraryResult<MetadataSourcePreview> {
    let current = owner(conn, movie, id).await?;
    let old_provider = current
        .metadata_provider
        .parse::<MetadataProvider>()
        .map_err(|_| LibraryError::invalid("provider", "Unknown current catalogue."))?;
    let old_identity = identity(old_provider, &current.metadata_id);
    let new_identity = identity(info.provider, &info.id);
    let external: Vec<(String, String)> = sqlx::query_as(if movie {
        "SELECT kind, external_id FROM movie_external_ids WHERE movie_id = ? ORDER BY kind, external_id"
    } else {
        "SELECT kind, external_id FROM series_external_ids WHERE series_id = ? ORDER BY kind, external_id"
    }).bind(id).fetch_all(&mut *conn).await?;
    let mut old_ids = external
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<HashSet<_>>();
    old_ids.insert((old_identity.kind.to_string(), old_identity.value.clone()));
    let mut new_ids = info.external_ids.clone();
    new_ids.push(new_identity);
    let verified = new_ids
        .iter()
        .any(|x| old_ids.contains(&(x.kind.to_string(), x.value.clone())));
    let mut blockers = Vec::new();
    if !verified && !req.confirm_identity {
        blockers.push("No shared external identity. Confirm that the target is the same title before applying.".into());
    }
    // Check every identity, not just the first match (which may be the current title).
    for ext in &new_ids {
        let other: bool = sqlx::query_scalar(if movie {
            "SELECT EXISTS(SELECT 1 FROM movie_external_ids WHERE kind = ? AND external_id = ? AND movie_id != ?)"
        } else {
            "SELECT EXISTS(SELECT 1 FROM series_external_ids WHERE kind = ? AND external_id = ? AND series_id != ?)"
        }).bind(ext.kind.as_str()).bind(&ext.value).bind(id).fetch_one(&mut *conn).await?;
        if other {
            blockers.push(
                "The target identity is already associated with another library title.".into(),
            );
            break;
        }
    }
    let duplicate: bool = sqlx::query_scalar(if movie {
        "SELECT EXISTS(SELECT 1 FROM movies WHERE metadata_provider = ? AND metadata_id = ? AND id != ?)"
    } else {
        "SELECT EXISTS(SELECT 1 FROM series WHERE metadata_provider = ? AND metadata_id = ? AND id != ?)"
    }).bind(info.provider.as_str()).bind(&info.id).bind(id).fetch_one(&mut *conn).await?;
    if duplicate {
        blockers.push("The target catalogue entry already owns another library title.".into());
    }
    let existing: Vec<Episode> = if movie {
        vec![]
    } else {
        sqlx::query_as("SELECT id, season_number, episode_number, title, air_date, monitored, episode_file_id, provider_episode_id, EXISTS(SELECT 1 FROM retained_metadata_episodes r WHERE r.episode_id = episodes.id) AS retained FROM episodes WHERE series_id = ? ORDER BY id")
            .bind(id).fetch_all(&mut *conn).await?
    };
    let mut coords = HashSet::new();
    for e in &info.episodes {
        if e.season_number < 0
            || e.episode_number < 0
            || !coords.insert((e.season_number, e.episode_number))
        {
            return Err(LibraryError::invalid(
                "provider",
                "The target catalogue has invalid or duplicate episode numbers.",
            ));
        }
    }
    let target_episodes: Vec<SourceTargetEpisode> = if movie {
        vec![]
    } else {
        info.episodes
            .iter()
            .map(|e| SourceTargetEpisode {
                title: e.title.clone(),
                season_number: e.season_number,
                episode_number: e.episode_number,
            })
            .collect()
    };
    let mut explicit = HashMap::new();
    for m in &req.mappings {
        if !existing.iter().any(|e| e.id == m.episode_id)
            || !coords.contains(&(m.season_number, m.episode_number))
            || explicit.insert(m.episode_id, m).is_some()
        {
            return Err(LibraryError::invalid(
                "mappings",
                "Choose each existing episode once and a valid target episode.",
            ));
        }
    }
    let clean = |s: Option<&str>| {
        s.map(kitsunarr_domain::text::clean_title)
            .filter(|s| !s.is_empty() && !s.starts_with("episode") && !s.starts_with("episodio"))
    };
    let mut taken = HashSet::new();
    let mut episodes = Vec::new();
    for old in &existing {
        let title = clean(old.title.as_deref());
        let old_title_unique = title.is_some()
            && existing
                .iter()
                .filter(|e| clean(e.title.as_deref()) == title)
                .count()
                == 1;
        let old_date_unique = old.air_date.is_some()
            && existing
                .iter()
                .filter(|e| e.air_date == old.air_date)
                .count()
                == 1;
        let candidates: Vec<_> = info
            .episodes
            .iter()
            .filter(|e| {
                (old_provider == info.provider
                    && old.provider_episode_id.is_some()
                    && old.provider_episode_id == e.provider_episode_id)
                    || (old_title_unique && clean(e.title.as_deref()) == title)
                    || (old_date_unique && e.air_date == old.air_date)
            })
            .collect();
        let target = if let Some(m) = explicit.get(&old.id) {
            Some((*m).clone())
        } else if candidates.len() == 1 {
            Some(SourceEpisodeMapping {
                episode_id: old.id,
                season_number: candidates[0].season_number,
                episode_number: candidates[0].episode_number,
            })
        } else {
            None
        };
        if let Some(m) = &target
            && !taken.insert((m.season_number, m.episode_number))
        {
            blockers.push(format!(
                "More than one existing episode maps to S{:02}E{:02}.",
                m.season_number, m.episode_number
            ));
        }
        episodes.push(SourceEpisodePreview {
            episode_id: old.id,
            season_number: old.season_number,
            episode_number: old.episode_number,
            title: old.title.clone(),
            target,
        });
    }
    for e in &episodes {
        if e.target.is_none() && coords.contains(&(e.season_number, e.episode_number)) {
            blockers.push(format!("S{:02}E{:02} needs an explicit episode match before its number can be used by the new catalogue.",e.season_number,e.episode_number));
        }
    }
    let fingerprint = serde_json::to_vec(&(
        &current,
        &external,
        &existing,
        info,
        req.confirm_identity,
        &req.mappings,
    ))
    .map_err(|e| LibraryError::invalid("revision", e.to_string()))?;
    let matched = episodes.iter().filter(|e| e.target.is_some()).count();
    Ok(MetadataSourcePreview {
        provider: info.provider,
        id: info.id.clone(),
        title: info.title.clone(),
        year: info.year,
        identity_verified: verified,
        revision: hex::encode(Sha256::digest(fingerprint)),
        blockers,
        matched_episodes: matched,
        retained_episodes: episodes.len() - matched,
        new_episodes: target_episodes.len().saturating_sub(taken.len()),
        episodes,
        target_episodes,
    })
}

/// Applies only an unchanged, successful preview. No file is moved, renamed or deleted.
pub async fn apply(
    db: &Database,
    metadata: &Metadata,
    movie: bool,
    id: i64,
    req: &MetadataSourceRequest,
) -> LibraryResult<RefreshResult> {
    if req.revision.as_deref().is_none_or(str::is_empty) {
        return Err(LibraryError::invalid(
            "revision",
            "Preview the catalogue change before applying it.",
        ));
    }
    let mut info = fetch(db, metadata, movie, id, req).await?;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let preview = plan(&mut tx, movie, id, req, &info).await?;
    if req.revision.as_deref() != Some(&preview.revision) {
        return Err(LibraryError::invalid(
            "revision",
            "The title or catalogue changed. Preview the change again.",
        ));
    }
    if !preview.blockers.is_empty() {
        return Err(LibraryError::invalid(
            "provider",
            preview.blockers.join(" "),
        ));
    }
    let current = owner(&mut tx, movie, id).await?;
    let old_provider = current
        .metadata_provider
        .parse::<MetadataProvider>()
        .map_err(|_| LibraryError::invalid("provider", "Unknown current catalogue."))?;
    info.external_ids
        .push(identity(old_provider, &current.metadata_id));
    info.external_ids.push(identity(info.provider, &info.id));
    if info.overview.as_deref().is_none_or(|s| s.trim().is_empty()) {
        info.overview = current.overview;
    }
    if info.images.is_empty() {
        info.images = serde_json::from_str(&current.images).unwrap_or_default();
    }
    let result = if movie {
        sqlx::query("UPDATE movies SET metadata_provider = ?, metadata_id = ? WHERE id = ?")
            .bind(info.provider.as_str())
            .bind(&info.id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        movies::update_metadata(
            &mut tx,
            id,
            &info.title,
            info.original_title.as_deref(),
            info.year,
            crate::movie_status(info.status),
            info.overview.as_deref(),
            info.network.as_deref(),
            info.runtime,
            &info.genres,
            &info.images,
            info.first_aired.as_deref(),
            &info.external_ids,
        )
        .await?;
        movies::set_original_country(&mut tx, id, info.original_country.as_deref()).await?;
        kitsunarr_db::auto_tags::apply_movie(&mut tx, id).await?;
        RefreshResult {
            added_episodes: 0,
            updated_episodes: 0,
        }
    } else {
        let monitor_new: bool = sqlx::query_scalar(
            "SELECT monitored AND monitor_new_items = 'all' FROM series WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        // Free coordinates before assigning them, preserving episode row ids and every FK.
        sqlx::query("UPDATE episodes SET episode_number = -id, provider_episode_id = NULL, absolute_episode_number = NULL, scene_season_number = NULL, scene_episode_number = NULL, scene_absolute_episode_number = NULL, unverified_scene_numbering = 0 WHERE series_id = ?")
            .bind(id).execute(&mut *tx).await?;
        for e in &preview.episodes {
            if e.target.is_some() {
                sqlx::query("DELETE FROM retained_metadata_episodes WHERE episode_id = ?")
                    .bind(e.episode_id)
                    .execute(&mut *tx)
                    .await?;
            } else {
                sqlx::query(
                    "INSERT OR IGNORE INTO retained_metadata_episodes (episode_id) VALUES (?)",
                )
                .bind(e.episode_id)
                .execute(&mut *tx)
                .await?;
            }
            let (s, n) = e
                .target
                .as_ref()
                .map(|m| (m.season_number, m.episode_number))
                .unwrap_or((e.season_number, e.episode_number));
            series::ensure_season(&mut tx, id, s, monitor_new && s > 0, None).await?;
            sqlx::query("UPDATE episodes SET season_number = ?, episode_number = ? WHERE id = ?")
                .bind(s)
                .bind(n)
                .bind(e.episode_id)
                .execute(&mut *tx)
                .await?;
        }
        // TheXEM aliases refer to the previous catalogue's seasons. User aliases survive.
        sqlx::query("DELETE FROM series_alternate_titles WHERE series_id = ? AND source = 'xem'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE series SET metadata_provider = ?, metadata_id = ?, episode_ordering = NULL WHERE id = ?")
            .bind(info.provider.as_str()).bind(&info.id).bind(id).execute(&mut *tx).await?;
        crate::merge_series_info(&mut tx, id, &info, monitor_new).await?
    };
    tx.commit().await?;
    Ok(result)
}
