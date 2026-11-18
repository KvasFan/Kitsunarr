//! The moment.js date formats Sonarr stores in Settings › UI (`MMM D YYYY`, `dddd, MMMM D
//! YYYY`, `h(:mm)a` …), rendered for a local date and time. Names are English, as moment's
//! default locale.

use chrono::{Datelike, NaiveDateTime, Timelike};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Tokens, longest first so `MMMM` wins over `MM`.
const TOKENS: [&str; 22] = [
    "YYYY", "MMMM", "dddd", "MMM", "ddd", "YY", "MM", "Do", "DD", "dd", "HH", "hh", "mm", "ss",
    "M", "D", "H", "h", "m", "s", "A", "a",
];

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn token(t: &str, dt: &NaiveDateTime) -> String {
    let hour12 = match dt.hour() % 12 {
        0 => 12,
        h => h,
    };
    let weekday = dt.weekday().num_days_from_monday() as usize;
    let month = dt.month0() as usize;
    match t {
        "YYYY" => format!("{:04}", dt.year()),
        "YY" => format!("{:02}", dt.year().rem_euclid(100)),
        "MMMM" => MONTHS[month].to_string(),
        "MMM" => MONTHS[month][..3].to_string(),
        "MM" => format!("{:02}", dt.month()),
        "M" => dt.month().to_string(),
        "DD" => format!("{:02}", dt.day()),
        "Do" => ordinal(dt.day()),
        "D" => dt.day().to_string(),
        "dddd" => DAYS[weekday].to_string(),
        "ddd" => DAYS[weekday][..3].to_string(),
        "dd" => DAYS[weekday][..2].to_string(),
        "HH" => format!("{:02}", dt.hour()),
        "H" => dt.hour().to_string(),
        "hh" => format!("{hour12:02}"),
        "h" => hour12.to_string(),
        "mm" => format!("{:02}", dt.minute()),
        "m" => dt.minute().to_string(),
        "ss" => format!("{:02}", dt.second()),
        "s" => dt.second().to_string(),
        "A" => if dt.hour() < 12 { "AM" } else { "PM" }.to_string(),
        "a" => if dt.hour() < 12 { "am" } else { "pm" }.to_string(),
        _ => String::new(),
    }
}

/// Renders a moment.js format; `[text]` is literal, other characters are kept.
pub fn format(dt: &NaiveDateTime, pattern: &str) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('[') {
            let end = inner.find(']').unwrap_or(inner.len());
            out.push_str(&inner[..end]);
            rest = inner.get(end + 1..).unwrap_or("");
            continue;
        }
        if let Some(t) = TOKENS.iter().find(|t| rest.starts_with(**t)) {
            out.push_str(&token(t, dt));
            rest = &rest[t.len()..];
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Sonarr's `formatTime`: in `h(:mm)a` the bracketed minutes show only when not zero.
pub fn format_time(dt: &NaiveDateTime, pattern: &str) -> String {
    let pattern = if dt.minute() == 0 {
        pattern.replace("(:mm)", "")
    } else {
        pattern.replace(['(', ')'], "")
    };
    format(dt, &pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn sonarr_formats_render_like_moment() {
        let d = at("2014-03-25 17:30");
        assert_eq!(format(&d, "MMM D YYYY"), "Mar 25 2014");
        assert_eq!(format(&d, "DD MMM YYYY"), "25 Mar 2014");
        assert_eq!(format(&d, "MM/D/YYYY"), "03/25/2014");
        assert_eq!(format(&d, "DD/MM/YYYY"), "25/03/2014");
        assert_eq!(format(&d, "YYYY-MM-DD"), "2014-03-25");
        assert_eq!(format(&d, "dddd, MMMM D YYYY"), "Tuesday, March 25 2014");
        assert_eq!(format(&d, "dddd, D MMMM YYYY"), "Tuesday, 25 March 2014");
        assert_eq!(format(&d, "ddd M/D"), "Tue 3/25");
        assert_eq!(format(&d, "[Week] Do"), "Week 25th");
        assert_eq!(format_time(&d, "h(:mm)a"), "5:30pm");
        assert_eq!(format_time(&at("2014-03-25 17:00"), "h(:mm)a"), "5pm");
        assert_eq!(format_time(&at("2014-03-25 00:05"), "h(:mm)a"), "12:05am");
        assert_eq!(format_time(&d, "HH:mm"), "17:30");
    }
}
