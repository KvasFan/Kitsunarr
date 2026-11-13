//! Sonarr's iCal feed (`CalendarFeedController`, `/feed/v3/calendar/Sonarr.ics`): episodes
//! airing from `pastDays` ago to `futureDays` ahead, optionally only premieres, tagged series or
//! including unmonitored ones, as timed or all-day events. Calendar apps subscribe with the API
//! key in the query (`?apikey=`).

use axum::Router;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::AppState;
use crate::error::ApiResult;

pub fn feed_router() -> Router<AppState> {
    Router::new()
        .route("/calendar/Sonarr.ics", get(calendar_feed))
        .route("/calendar/Kitsunarr.ics", get(calendar_feed))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FeedQuery {
    past_days: i64,
    future_days: i64,
    /// Tag labels, comma separated.
    tags: String,
    unmonitored: bool,
    premieres_only: bool,
    as_all_day: bool,
}

impl Default for FeedQuery {
    fn default() -> Self {
        Self {
            past_days: 7,
            future_days: 28,
            tags: String::new(),
            unmonitored: false,
            premieres_only: false,
            as_all_day: false,
        }
    }
}

/// An episode as the feed writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEpisode {
    pub episode_id: i64,
    pub series_title: String,
    pub series_type: String,
    pub network: Option<String>,
    pub runtime: Option<i32>,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date_utc: String,
    pub has_file: bool,
}

/// RFC 5545 text escaping.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// RFC 5545 line folding (75 octets, continuation lines start with a space).
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

/// The calendar document.
pub fn render(
    episodes: &[FeedEpisode],
    q: &FeedQuery,
    product: &str,
    now: DateTime<Utc>,
) -> String {
    let mut out = String::new();
    let name = format!("{product} TV Schedule");
    for line in [
        "BEGIN:VCALENDAR".to_string(),
        "PRODID:-//github.com/KvasFan/Kitsunarr//Kitsunarr//EN".to_string(),
        "VERSION:2.0".to_string(),
        format!("NAME:{}", escape(&name)),
        format!("X-WR-CALNAME:{}", escape(&name)),
    ] {
        fold(&line, &mut out);
    }
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    for e in episodes {
        if q.premieres_only && (e.season_number == 0 || e.episode_number != 1) {
            continue;
        }
        let Ok(air) = DateTime::parse_from_rfc3339(&e.air_date_utc).map(|d| d.with_timezone(&Utc))
        else {
            continue;
        };
        let title = e.title.clone().unwrap_or_default();
        let summary = if e.series_type == "daily" {
            format!("{} - {title}", e.series_title)
        } else {
            format!(
                "{} - {}x{:02} - {title}",
                e.series_title, e.season_number, e.episode_number
            )
        };
        let mut lines = vec!["BEGIN:VEVENT".to_string()];
        if let Some(n) = e.network.as_deref().filter(|n| !n.is_empty()) {
            lines.push(format!("CATEGORIES:{}", escape(n)));
        }
        if let Some(o) = e.overview.as_deref().filter(|o| !o.is_empty()) {
            lines.push(format!("DESCRIPTION:{}", escape(o)));
        }
        if q.as_all_day {
            lines.push(format!("DTSTART;VALUE=DATE:{}", air.format("%Y%m%d")));
        } else {
            let end = air + Duration::minutes(i64::from(e.runtime.unwrap_or(0)));
            lines.push(format!("DTEND:{}", end.format("%Y%m%dT%H%M%SZ")));
            lines.push(format!("DTSTART:{}", air.format("%Y%m%dT%H%M%SZ")));
        }
        lines.push(format!("DTSTAMP:{stamp}"));
        lines.push(format!(
            "STATUS:{}",
            if e.has_file { "CONFIRMED" } else { "TENTATIVE" }
        ));
        lines.push(format!("SUMMARY:{}", escape(&summary)));
        lines.push(format!("UID:NzbDrone_episode_{}", e.episode_id));
        lines.push("END:VEVENT".to_string());
        for l in lines {
            fold(&l, &mut out);
        }
    }
    fold("END:VCALENDAR", &mut out);
    out
}

async fn calendar_feed(
    State(state): State<AppState>,
    Query(q): Query<FeedQuery>,
) -> ApiResult<Response> {
    let today = Utc::now().date_naive();
    let start = (today - Duration::days(q.past_days.max(0)))
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc();
    let end = (today + Duration::days(q.future_days.max(0)))
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc();
    let labels: Vec<String> = q
        .tags
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let tag_ids: Vec<i64> = if labels.is_empty() {
        Vec::new()
    } else {
        kitsunarr_db::tags::list(state.db.pool())
            .await?
            .into_iter()
            .filter(|t| labels.contains(&t.label.to_lowercase()))
            .map(|t| t.id)
            .collect()
    };
    type Row = (
        i64,
        String,
        String,
        Option<String>,
        Option<i32>,
        i32,
        i32,
        Option<String>,
        Option<String>,
        String,
        bool,
    );
    let rows: Vec<Row> = kitsunarr_db::sqlx::query_as(
        "SELECT e.id AS episode_id, s.title AS series_title, s.series_type, s.network, s.runtime, \
           e.season_number, e.episode_number, e.title, e.overview, e.air_date_utc, \
           e.episode_file_id IS NOT NULL AS has_file \
         FROM episodes e JOIN series s ON s.id = e.series_id \
         WHERE e.air_date_utc >= ? AND e.air_date_utc < ? AND (? OR (e.monitored = 1 AND s.monitored = 1)) \
         ORDER BY e.air_date_utc LIMIT 10000",
    )
    .bind(start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    .bind(end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    .bind(q.unmonitored)
    .fetch_all(state.db.pool())
    .await
    .map_err(kitsunarr_db::DbError::from)?;
    let mut episodes: Vec<FeedEpisode> = rows
        .into_iter()
        .map(|r| FeedEpisode {
            episode_id: r.0,
            series_title: r.1,
            series_type: r.2,
            network: r.3,
            runtime: r.4,
            season_number: r.5,
            episode_number: r.6,
            title: r.7,
            overview: r.8,
            air_date_utc: r.9,
            has_file: r.10,
        })
        .collect();
    if !tag_ids.is_empty() {
        let tagged: Vec<i64> = kitsunarr_db::sqlx::query_scalar(
            "SELECT e.id FROM episodes e JOIN series_tags t ON t.series_id = e.series_id \
             WHERE t.tag_id IN (SELECT value FROM json_each(?))",
        )
        .bind(serde_json::to_string(&tag_ids).unwrap_or_default())
        .fetch_all(state.db.pool())
        .await
        .map_err(kitsunarr_db::DbError::from)?;
        episodes.retain(|e| tagged.contains(&e.episode_id));
    }
    let body = render(&episodes, &q, "Kitsunarr", Utc::now());
    Ok((
        [(header::CONTENT_TYPE, "text/calendar; charset=utf-8")],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(n: i32, kind: &str) -> FeedEpisode {
        FeedEpisode {
            episode_id: 7,
            series_title: "Breaking Bad".into(),
            series_type: kind.into(),
            network: Some("AMC".into()),
            runtime: Some(47),
            season_number: 1,
            episode_number: n,
            title: Some("Pilot".into()),
            overview: Some("A teacher, a lab; a van.".into()),
            air_date_utc: "2008-01-21T03:00:00Z".into(),
            has_file: true,
        }
    }

    #[test]
    fn events_follow_calendar_feed_controller() {
        let now = DateTime::parse_from_rfc3339("2026-11-13T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ics = render(
            &[episode(1, "standard")],
            &FeedQuery::default(),
            "Kitsunarr",
            now,
        );
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\nPRODID:-//github.com/KvasFan/Kitsunarr//Kitsunarr//EN\r\nVERSION:2.0\r\nNAME:Kitsunarr TV Schedule\r\n"));
        for line in [
            "CATEGORIES:AMC",
            "DESCRIPTION:A teacher\\, a lab\\; a van.",
            "DTEND:20080121T034700Z",
            "DTSTART:20080121T030000Z",
            "DTSTAMP:20261113T100000Z",
            "STATUS:CONFIRMED",
            "SUMMARY:Breaking Bad - 1x01 - Pilot",
            "UID:NzbDrone_episode_7",
        ] {
            assert!(ics.contains(&format!("{line}\r\n")), "{line}\n{ics}");
        }
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        let all_day = FeedQuery {
            as_all_day: true,
            premieres_only: true,
            ..Default::default()
        };
        let ics = render(
            &[episode(1, "daily"), episode(2, "daily")],
            &all_day,
            "Kitsunarr",
            now,
        );
        assert!(ics.contains("DTSTART;VALUE=DATE:20080121\r\n"));
        assert!(!ics.contains("DTEND"));
        assert!(ics.contains("SUMMARY:Breaking Bad - Pilot\r\n"));
        assert_eq!(ics.matches("BEGIN:VEVENT").count(), 1, "premieres only");
        // Long lines are folded at 75 octets.
        let mut long = episode(1, "standard");
        long.overview = Some("x".repeat(200));
        let ics = render(&[long], &FeedQuery::default(), "Kitsunarr", now);
        assert!(ics.lines().all(|l| l.len() <= 75));
    }
}
