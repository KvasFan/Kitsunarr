//! Watched progress to import lists: Tanukarr's `ImportListProgressService` and
//! `ImportListProgressGuard`.
//!
//! After a Jellyfin watched sync (with "sync list progress" on), the contiguous watched prefix of
//! each matched series is written to the user's own AniList, MyAnimeList, Trakt or Simkl list
//! when the list holds the title and has its own "sync watched progress" on. Progress only ever
//! grows; each (list, series) pair is a durable queue row persisted before any request, with
//! back-off on failure and a global budget of 10 writes a minute.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use kitsunarr_db::Database;
use kitsunarr_db::providers::{self, ProviderRecord};
use kitsunarr_domain::api::settings::ProviderKind;
use kitsunarr_importlists::ListError;
use kitsunarr_importlists::progress::{
    self, ItemIds, ProgressEpisode, ProgressError, SeriesRef, Target,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Writes allowed per window (Tanukarr's budget).
pub const BUDGET: usize = 10;
const WINDOW: Duration = Duration::from_secs(60);

/// The fixed one-minute window: (start, writes in it).
static BUDGET_WINDOW: Mutex<Option<(Instant, usize)>> = Mutex::new(None);
/// One sync at a time (Tanukarr's global lock).
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn take_budget() -> bool {
    let mut w = BUDGET_WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match &mut *w {
        Some((start, used)) if now.duration_since(*start) < WINDOW => {
            if *used >= BUDGET {
                return false;
            }
            *used += 1;
        }
        _ => *w = Some((now, 1)),
    }
    true
}

/// An episode as the guard sees it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct WatchEpisode {
    pub id: i64,
    pub season_number: i64,
    pub episode_number: i64,
    pub provider_episode_id: Option<String>,
    pub watched: bool,
    pub watched_source: Option<String>,
    pub watched_at: Option<String>,
}

/// `ImportListProgressGuard.ContiguousProgress`: watched episodes counted from S01E01 in strict
/// order, stopping at the first gap or unwatched episode. Specials are ignored; only
/// TheTVDB-owned series (`multiple_seasons`) may roll over into the next season at E1, and other
/// series must have season 1 only.
pub fn contiguous_progress(episodes: &[WatchEpisode], multiple_seasons: bool) -> usize {
    let mut native: Vec<&WatchEpisode> = episodes.iter().filter(|e| e.season_number > 0).collect();
    native.sort_by_key(|e| (e.season_number, e.episode_number));
    if native.is_empty()
        || native.iter().any(|e| e.episode_number <= 0)
        || native.windows(2).any(|w| {
            (w[0].season_number, w[0].episode_number) == (w[1].season_number, w[1].episode_number)
        })
        || (!multiple_seasons && native.iter().any(|e| e.season_number != 1))
    {
        return 0;
    }
    let (mut season, mut next) = (1, 1);
    let mut count = 0;
    for e in native {
        let in_order = (e.season_number == season && e.episode_number == next)
            || (multiple_seasons
                && count > 0
                && e.season_number == season + 1
                && e.episode_number == 1);
        if !in_order || !e.watched {
            break;
        }
        season = e.season_number;
        next = e.episode_number + 1;
        count += 1;
    }
    count
}

/// The identity of the first `n` episodes: progress acknowledged for other episodes (a
/// renumbering, a replaced catalogue) starts over.
fn identity(episodes: &[WatchEpisode], n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let mut h = Sha256::new();
    for e in episodes.iter().take(n) {
        h.update(format!(
            "{}:{}:{}:{}\n",
            e.id,
            e.season_number,
            e.episode_number,
            e.provider_episode_id.as_deref().unwrap_or_default()
        ));
    }
    hex::encode(h.finalize())
}

fn fingerprint(list: &ProviderRecord, target: Target, source: &str) -> String {
    let user = ["username", "authUser", "baseUrl"]
        .iter()
        .map(|k| {
            list.settings
                .get(*k)
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("|");
    hex::encode(Sha256::digest(
        format!(
            "{}\n{}\n{:?}\n{}\n{}\n{}",
            list.id, list.implementation, target.kind, target.remote_id, user, source
        )
        .as_bytes(),
    ))
}

/// Who may still write: re-checked before every request.
#[derive(Debug, Clone)]
pub struct Source {
    /// The Jellyfin connection whose watched state is written; `None` skips the connection
    /// check (the caller vouches for it).
    pub connection_id: Option<i64>,
    /// The connection's fingerprint, as stored in `episodes.watched_source`.
    pub fingerprint: String,
}

fn writes_progress(list: &ProviderRecord) -> bool {
    list.enabled
        && list
            .options
            .get("enableAutomaticAdd")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        && progress::enabled(&list.implementation, &list.settings)
}

struct SeriesInfo {
    provider: String,
    metadata_id: String,
    ordering: Option<String>,
}

async fn target_of(
    db: &Database,
    list: &ProviderRecord,
    series: &SeriesInfo,
) -> Result<Option<Target>, String> {
    let items = kitsunarr_db::import_lists::items(db.pool(), Some(list.id))
        .await
        .map_err(|e| e.to_string())?;
    let s = SeriesRef {
        provider: &series.provider,
        metadata_id: &series.metadata_id,
        ordered: series.ordering.is_some(),
    };
    Ok(items.iter().find_map(|i| {
        progress::target(
            &list.implementation,
            s,
            ItemIds {
                tvdb: i.tvdb_id,
                mal: i.mal_id,
                anilist: i.anilist_id,
            },
        )
    }))
}

#[derive(sqlx::FromRow)]
struct State {
    target_fingerprint: String,
    pending_progress: i64,
    acknowledged_progress: i64,
    acknowledged_identity: String,
    failure_count: i64,
    next_attempt_at: String,
}

fn stamp(t: chrono::DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `SyncProgress`: queues and writes the series' watched progress to every list that holds it.
/// Returns how many writes were confirmed.
pub async fn sync_series(
    db: &Database,
    http: &reqwest::Client,
    series_id: i64,
    source: &Source,
) -> Result<usize, String> {
    let _guard = LOCK.lock().await;
    let err = |e: sqlx::Error| e.to_string();
    let Some((provider, metadata_id, ordering)) =
        sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT metadata_provider, metadata_id, episode_ordering FROM series WHERE id = ?",
        )
        .bind(series_id)
        .fetch_optional(db.pool())
        .await
        .map_err(err)?
    else {
        return Ok(0);
    };
    let series = SeriesInfo {
        provider,
        metadata_id,
        ordering,
    };
    let mut episodes: Vec<WatchEpisode> = sqlx::query_as(
        "SELECT id, season_number, episode_number, provider_episode_id, watched, watched_source, watched_at \
         FROM episodes WHERE series_id = ? AND season_number > 0 ORDER BY season_number, episode_number",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await
    .map_err(err)?;
    episodes.sort_by_key(|e| (e.season_number, e.episode_number));
    let count = contiguous_progress(&episodes, series.provider.eq_ignore_ascii_case("tvdb"));
    if count == 0 {
        return Ok(0);
    }
    // Every watched episode must come from the one source.
    let sources: std::collections::HashSet<&str> = episodes
        .iter()
        .filter(|e| e.watched)
        .map(|e| e.watched_source.as_deref().unwrap_or_default())
        .collect();
    if sources.len() != 1 || !sources.contains(source.fingerprint.as_str()) {
        return Ok(0);
    }
    let prefix: Vec<ProgressEpisode> = episodes
        .iter()
        .take(count)
        .map(|e| ProgressEpisode {
            season: e.season_number,
            episode: e.episode_number,
            provider_id: e
                .provider_episode_id
                .as_deref()
                .and_then(|v| v.parse().ok()),
            watched_at: e.watched_at.clone(),
        })
        .collect();
    let lists = providers::list(db.pool(), ProviderKind::ImportList)
        .await
        .map_err(|e| e.to_string())?;
    let mut confirmed = 0;
    for list in lists.iter().filter(|l| writes_progress(l)) {
        let Some(target) = target_of(db, list, &series).await? else {
            continue;
        };
        match dispatch(
            db, http, series_id, &series, list, target, source, &episodes, &prefix,
        )
        .await
        {
            Ok(Dispatch::Written) => confirmed += 1,
            Ok(Dispatch::Idle) => {}
            Ok(Dispatch::OutOfBudget) => break,
            Err(e) => {
                tracing::warn!(list = %list.name, series_id, error = %e, "list progress was not written")
            }
        }
    }
    Ok(confirmed)
}

enum Dispatch {
    Written,
    Idle,
    OutOfBudget,
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    db: &Database,
    http: &reqwest::Client,
    series_id: i64,
    series: &SeriesInfo,
    list: &ProviderRecord,
    target: Target,
    source: &Source,
    episodes: &[WatchEpisode],
    prefix: &[ProgressEpisode],
) -> Result<Dispatch, String> {
    let err = |e: sqlx::Error| e.to_string();
    let progress_now = prefix.len() as i64;
    let fp = fingerprint(list, target, &source.fingerprint);
    let now = Utc::now();
    let existing: Option<State> = sqlx::query_as(
        "SELECT target_fingerprint, pending_progress, acknowledged_progress, acknowledged_identity, failure_count, next_attempt_at \
         FROM import_list_progress_states WHERE import_list_id = ? AND series_id = ?",
    )
    .bind(list.id)
    .bind(series_id)
    .fetch_optional(db.pool())
    .await
    .map_err(err)?;
    let mut st = existing.unwrap_or(State {
        target_fingerprint: fp.clone(),
        pending_progress: 0,
        acknowledged_progress: 0,
        acknowledged_identity: String::new(),
        failure_count: 0,
        next_attempt_at: stamp(now),
    });
    let ack = st.acknowledged_progress.max(0) as usize;
    let reset = st.target_fingerprint != fp
        || (ack > 0
            && (ack > episodes.len() || identity(episodes, ack) != st.acknowledged_identity));
    if reset {
        st = State {
            target_fingerprint: fp.clone(),
            pending_progress: 0,
            acknowledged_progress: 0,
            acknowledged_identity: String::new(),
            failure_count: 0,
            next_attempt_at: stamp(now),
        };
    }
    st.pending_progress = st.pending_progress.max(progress_now);
    let save = |st: &State, last_attempt: Option<String>, error: Option<String>| {
        sqlx::query(
            "INSERT INTO import_list_progress_states (import_list_id, series_id, target_fingerprint, pending_progress, \
             acknowledged_progress, acknowledged_identity, failure_count, next_attempt_at, last_attempt_at, last_error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT (import_list_id, series_id) DO UPDATE SET \
             target_fingerprint = ?3, pending_progress = ?4, acknowledged_progress = ?5, acknowledged_identity = ?6, \
             failure_count = ?7, next_attempt_at = ?8, last_attempt_at = COALESCE(?9, last_attempt_at), last_error = ?10",
        )
        .bind(list.id)
        .bind(series_id)
        .bind(st.target_fingerprint.clone())
        .bind(st.pending_progress)
        .bind(st.acknowledged_progress)
        .bind(st.acknowledged_identity.clone())
        .bind(st.failure_count)
        .bind(st.next_attempt_at.clone())
        .bind(last_attempt)
        .bind(error)
        .execute(db.pool())
    };
    // Pending progress is durable before anything else happens.
    save(&st, None, None).await.map_err(err)?;
    let due = chrono::DateTime::parse_from_rfc3339(&st.next_attempt_at).map_or(true, |t| t <= now);
    if st.acknowledged_progress >= st.pending_progress || !due {
        return Ok(Dispatch::Idle);
    }
    if !take_budget() {
        return Ok(Dispatch::OutOfBudget);
    }
    // Re-check everything a request depends on.
    if let Some(connection_id) = source.connection_id
        && !crate::jellyfin::progress_authorized(db, connection_id, &source.fingerprint, series_id)
            .await
    {
        return Ok(Dispatch::Idle);
    }
    let Ok(current) = providers::get(db.pool(), ProviderKind::ImportList, list.id).await else {
        return Ok(Dispatch::Idle);
    };
    if current.implementation != list.implementation
        || !writes_progress(&current)
        || target_of(db, &current, series).await? != Some(target)
        || fingerprint(&current, target, &source.fingerprint) != fp
    {
        return Ok(Dispatch::Idle);
    }
    st.next_attempt_at = stamp(now + chrono::Duration::minutes(5));
    save(&st, Some(stamp(now)), None).await.map_err(err)?;
    let result = progress::advance(
        http,
        &current.implementation,
        &current.settings,
        target,
        st.acknowledged_progress,
        st.pending_progress,
        prefix,
    )
    .await;
    let now = Utc::now();
    match result {
        Ok(confirmed) if confirmed > 0 => {
            st.acknowledged_progress = st.acknowledged_progress.max(confirmed.min(progress_now));
            st.acknowledged_identity = identity(episodes, st.acknowledged_progress as usize);
            st.failure_count = 0;
            st.next_attempt_at = stamp(if st.acknowledged_progress >= st.pending_progress {
                now
            } else {
                now + chrono::Duration::minutes(1)
            });
            save(&st, None, None).await.map_err(err)?;
            Ok(Dispatch::Written)
        }
        other => {
            let message = match &other {
                Ok(_) => "the list service confirmed no progress".to_string(),
                Err(e) => e.to_string(),
            };
            let n = (st.failure_count + 1).min(16);
            st.failure_count = n;
            let mut wait = chrono::Duration::minutes((5i64 << (n - 1).min(7)).min(360));
            match &other {
                Err(ProgressError::Unavailable(_)) => wait = chrono::Duration::days(1),
                Err(ProgressError::List(ListError::RateLimited(Some(d)))) => {
                    let d = chrono::Duration::seconds(d.as_secs().min(86_400) as i64);
                    wait = wait.max(d);
                }
                _ => {}
            }
            st.next_attempt_at = stamp(now + wait);
            save(&st, None, Some(message.clone())).await.map_err(err)?;
            Err(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(season: i64, episode: i64, watched: bool) -> WatchEpisode {
        WatchEpisode {
            id: season * 100 + episode,
            season_number: season,
            episode_number: episode,
            provider_episode_id: None,
            watched,
            watched_source: watched.then(|| "fp".into()),
            watched_at: None,
        }
    }

    /// Cases after Tanukarr's `ImportListProgressGuardFixture`.
    #[test]
    fn contiguous_progress_follows_the_guard() {
        assert_eq!(contiguous_progress(&[], false), 0);
        let eps = [
            ep(1, 1, true),
            ep(1, 2, true),
            ep(1, 3, false),
            ep(1, 4, true),
        ];
        assert_eq!(contiguous_progress(&eps, false), 2);
        // A gap stops the count.
        assert_eq!(
            contiguous_progress(&[ep(1, 1, true), ep(1, 3, true)], false),
            1
        );
        // Must start at E1.
        assert_eq!(contiguous_progress(&[ep(1, 2, true)], false), 0);
        // Specials are ignored.
        assert_eq!(
            contiguous_progress(&[ep(0, 1, false), ep(1, 1, true)], false),
            1
        );
        // Single-season catalogues refuse other seasons.
        let two = [ep(1, 1, true), ep(1, 2, true), ep(2, 1, true)];
        assert_eq!(contiguous_progress(&two, false), 0);
        // TheTVDB series roll over at E1.
        assert_eq!(contiguous_progress(&two, true), 3);
        assert_eq!(
            contiguous_progress(&[ep(1, 1, true), ep(2, 2, true)], true),
            1
        );
        // Episode numbers must be positive.
        assert_eq!(
            contiguous_progress(&[ep(1, 0, true), ep(1, 1, true)], false),
            0
        );
    }

    #[test]
    fn budget_allows_ten_a_minute() {
        *BUDGET_WINDOW.lock().unwrap() = None;
        for _ in 0..BUDGET {
            assert!(take_budget());
        }
        assert!(!take_budget());
        *BUDGET_WINDOW.lock().unwrap() = Some((Instant::now() - WINDOW, BUDGET));
        assert!(take_budget());
        *BUDGET_WINDOW.lock().unwrap() = None;
    }
}
