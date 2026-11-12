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
/// When this process started (the start-up grace period), fixed when the database opens.
static STARTED: std::sync::LazyLock<DateTime<Utc>> = std::sync::LazyLock::new(Utc::now);

pub fn process_started() -> DateTime<Utc> {
    *STARTED
}

/// `MinimumTimeSinceStartup`.
const STARTUP_GRACE_MINUTES: i64 = 15;

/// Which providers' status (Sonarr's `IndexerStatusService` / `DownloadClientStatusService`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Indexer,
    /// Download clients: no escalation within 5 minutes of the first failure, at most 1 hour.
    DownloadClient,
}

impl Kind {
    fn max_level(self) -> i64 {
        match self {
            Self::Indexer => PERIODS.len() as i64 - 1,
            Self::DownloadClient => 5,
        }
    }

    fn initial_grace(self) -> Duration {
        match self {
            Self::Indexer => Duration::zero(),
            Self::DownloadClient => Duration::minutes(5),
        }
    }

    fn select_one(self) -> &'static str {
        match self {
            Self::Indexer => {
                "SELECT indexer_id, initial_failure, most_recent_failure, escalation_level, disabled_till \
                 FROM indexer_status WHERE indexer_id = ?"
            }
            Self::DownloadClient => {
                "SELECT download_client_id AS indexer_id, initial_failure, most_recent_failure, escalation_level, \
                 disabled_till FROM download_client_status WHERE download_client_id = ?"
            }
        }
    }

    fn select_all(self) -> &'static str {
        match self {
            Self::Indexer => {
                "SELECT indexer_id, initial_failure, most_recent_failure, escalation_level, disabled_till \
                 FROM indexer_status ORDER BY indexer_id"
            }
            Self::DownloadClient => {
                "SELECT download_client_id AS indexer_id, initial_failure, most_recent_failure, escalation_level, \
                 disabled_till FROM download_client_status ORDER BY download_client_id"
            }
        }
    }

    fn upsert(self) -> &'static str {
        match self {
            Self::Indexer => {
                "INSERT INTO indexer_status (indexer_id, initial_failure, most_recent_failure, escalation_level, disabled_till) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (indexer_id) DO UPDATE SET initial_failure = ?2, \
                 most_recent_failure = ?3, escalation_level = ?4, disabled_till = ?5"
            }
            Self::DownloadClient => {
                "INSERT INTO download_client_status (download_client_id, initial_failure, most_recent_failure, \
                 escalation_level, disabled_till) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (download_client_id) DO UPDATE SET \
                 initial_failure = ?2, most_recent_failure = ?3, escalation_level = ?4, disabled_till = ?5"
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, FromRow)]
pub struct IndexerStatus {
    /// The indexer's (or download client's) id.
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

fn period(kind: Kind, level: i64) -> Duration {
    Duration::seconds(PERIODS[level.clamp(0, kind.max_level()) as usize])
}

pub async fn get(pool: &SqlitePool, indexer_id: i64) -> DbResult<IndexerStatus> {
    get_of(Kind::Indexer, pool, indexer_id).await
}

pub async fn get_of(kind: Kind, pool: &SqlitePool, indexer_id: i64) -> DbResult<IndexerStatus> {
    Ok(sqlx::query_as(kind.select_one())
        .bind(indexer_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(IndexerStatus {
            indexer_id,
            ..Default::default()
        }))
}

pub async fn all(pool: &SqlitePool) -> DbResult<Vec<IndexerStatus>> {
    all_of(Kind::Indexer, pool).await
}

pub async fn all_of(kind: Kind, pool: &SqlitePool) -> DbResult<Vec<IndexerStatus>> {
    Ok(sqlx::query_as(kind.select_all()).fetch_all(pool).await?)
}

/// `GetBlockedProviders`: the indexers disabled at `now`.
pub async fn blocked(pool: &SqlitePool, now: DateTime<Utc>) -> DbResult<Vec<IndexerStatus>> {
    blocked_of(Kind::Indexer, pool, now).await
}

pub async fn blocked_of(
    kind: Kind,
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> DbResult<Vec<IndexerStatus>> {
    Ok(all_of(kind, pool)
        .await?
        .into_iter()
        .filter(|s| s.is_disabled(now))
        .collect())
}

async fn save(kind: Kind, pool: &SqlitePool, s: &IndexerStatus) -> DbResult<()> {
    // A provider deleted meanwhile has no status (the foreign key refuses it).
    let _ = sqlx::query(kind.upsert())
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
    record_success_of(Kind::Indexer, pool, indexer_id).await
}

pub async fn record_success_of(kind: Kind, pool: &SqlitePool, indexer_id: i64) -> DbResult<()> {
    let mut s = get_of(kind, pool, indexer_id).await?;
    if s.escalation_level == 0 {
        return Ok(());
    }
    s.escalation_level -= 1;
    s.disabled_till = None;
    save(kind, pool, &s).await
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
    record_failure_of(Kind::Indexer, pool, indexer_id, failure, started_at, now).await
}

pub async fn record_failure_of(
    kind: Kind,
    pool: &SqlitePool,
    indexer_id: i64,
    failure: Failure,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DbResult<IndexerStatus> {
    let max_level = kind.max_level();
    let mut s = get_of(kind, pool, indexer_id).await?;
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
    let in_grace = s
        .initial_failure
        .as_deref()
        .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
        .is_some_and(|d| d.with_timezone(&Utc) + kind.initial_grace() > now);
    if escalate && !in_grace && !in_startup_grace {
        s.escalation_level = (s.escalation_level + 1).min(max_level);
    }
    if let Some(minimum) = minimum {
        while s.escalation_level < max_level && period(kind, s.escalation_level) < minimum {
            s.escalation_level += 1;
        }
    }
    if !in_grace || minimum.is_some() {
        let mut till = now + period(kind, s.escalation_level);
        if in_startup_grace && minimum.is_none() {
            till = till.min(now + Duration::seconds(PERIODS[2]));
        }
        s.disabled_till = Some(stamp(till));
    }
    save(kind, pool, &s).await?;
    Ok(s)
}
