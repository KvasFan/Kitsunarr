//! Indexer failure back-off: Sonarr's `IndexerStatusService` (`ProviderStatusServiceBase`).
//!
//! Each failure raises the indexer's escalation level and disables it for the matching period;
//! each success lowers the level by one. Connection failures (name resolution, refused
//! connections) do not escalate; a rate limit disables the indexer for at least its
//! `Retry-After`. During the first 15 minutes after start-up nothing escalates and no failure
//! disables an indexer for more than 5 minutes.

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, SqlitePool};

use crate::DbResult;

/// `EscalationBackOff.Periods`, in seconds.
pub const PERIODS: [i64; 10] = [
    0,
    60,
    5 * 60,
    15 * 60,
    30 * 60,
    3600,
    3 * 3600,
    6 * 3600,
    12 * 3600,
    24 * 3600,
];
const MAX_LEVEL: i64 = PERIODS.len() as i64 - 1;
/// `MinimumTimeSinceStartup`.
const STARTUP_GRACE_MINUTES: i64 = 15;

#[derive(Debug, Clone, Default, PartialEq, Eq, FromRow)]
pub struct IndexerStatus {
    pub indexer_id: i64,
    pub initial_failure: Option<String>,
    pub most_recent_failure: Option<String>,
    pub escalation_level: i64,
    pub disabled_till: Option<String>,
}

impl IndexerStatus {
    pub fn disabled_till(&self) -> Option<DateTime<Utc>> {
        self.disabled_till
            .as_deref()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&Utc))
    }

    /// `IsDisabled`.
    pub fn is_disabled(&self, now: DateTime<Utc>) -> bool {
        self.disabled_till().is_some_and(|t| t > now)
    }
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn period(level: i64) -> Duration {
    Duration::seconds(PERIODS[level.clamp(0, MAX_LEVEL) as usize])
}

pub async fn get(pool: &SqlitePool, indexer_id: i64) -> DbResult<IndexerStatus> {
    Ok(sqlx::query_as(
        "SELECT indexer_id, initial_failure, most_recent_failure, escalation_level, disabled_till \
         FROM indexer_status WHERE indexer_id = ?",
    )
    .bind(indexer_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(IndexerStatus {
        indexer_id,
        ..Default::default()
    }))
}

pub async fn all(pool: &SqlitePool) -> DbResult<Vec<IndexerStatus>> {
    Ok(sqlx::query_as(
        "SELECT indexer_id, initial_failure, most_recent_failure, escalation_level, disabled_till \
         FROM indexer_status ORDER BY indexer_id",
    )
    .fetch_all(pool)
    .await?)
}

/// `GetBlockedProviders`: the indexers disabled at `now`.
pub async fn blocked(pool: &SqlitePool, now: DateTime<Utc>) -> DbResult<Vec<IndexerStatus>> {
    Ok(all(pool)
        .await?
        .into_iter()
        .filter(|s| s.is_disabled(now))
        .collect())
}

async fn save(pool: &SqlitePool, s: &IndexerStatus) -> DbResult<()> {
    // An indexer deleted meanwhile has no status (the foreign key refuses it).
    let _ = sqlx::query(
        "INSERT INTO indexer_status (indexer_id, initial_failure, most_recent_failure, escalation_level, disabled_till) \
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (indexer_id) DO UPDATE SET initial_failure = ?2, \
         most_recent_failure = ?3, escalation_level = ?4, disabled_till = ?5",
    )
    .bind(s.indexer_id)
    .bind(&s.initial_failure)
    .bind(&s.most_recent_failure)
    .bind(s.escalation_level)
    .bind(&s.disabled_till)
    .execute(pool)
    .await;
    Ok(())
}

/// `RecordSuccess`.
pub async fn record_success(pool: &SqlitePool, indexer_id: i64) -> DbResult<()> {
    let mut s = get(pool, indexer_id).await?;
    if s.escalation_level == 0 {
        return Ok(());
    }
    s.escalation_level -= 1;
    s.disabled_till = None;
    save(pool, &s).await
}

/// How a request failed, for [`record_failure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Any failure (`RecordFailure`).
    Failure,
    /// Name resolution or connection refused (`RecordConnectionFailure`): no escalation.
    Connection,
    /// A rate limit: disabled for at least this long.
    RateLimited(Duration),
}

/// `RecordFailure` / `RecordConnectionFailure`.
pub async fn record_failure(
    pool: &SqlitePool,
    indexer_id: i64,
    failure: Failure,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DbResult<IndexerStatus> {
    let mut s = get(pool, indexer_id).await?;
    let mut escalate = failure != Failure::Connection;
    let minimum = match failure {
        Failure::RateLimited(d) => Some(d),
        _ => None,
    };
    s.most_recent_failure = Some(stamp(now));
    if s.escalation_level == 0 {
        s.initial_failure = Some(stamp(now));
        s.escalation_level = 1;
        escalate = false;
    }
    let in_startup_grace = started_at + Duration::minutes(STARTUP_GRACE_MINUTES) > now;
    if escalate && !in_startup_grace {
        s.escalation_level = (s.escalation_level + 1).min(MAX_LEVEL);
    }
    if let Some(minimum) = minimum {
        while s.escalation_level < MAX_LEVEL && period(s.escalation_level) < minimum {
            s.escalation_level += 1;
        }
    }
    let mut till = now + period(s.escalation_level);
    if in_startup_grace && minimum.is_none() {
        till = till.min(now + Duration::seconds(PERIODS[2]));
    }
    s.disabled_till = Some(stamp(till));
    save(pool, &s).await?;
    Ok(s)
}
