//! Discord webhooks (Sonarr's `Discord`, `DiscordProxy`): one embed per event, with the fields
//! chosen in `grabFields` and `importFields`.

use serde_json::{Map, Value, json};

use crate::{Event, Instance, Notifier, NotifyError, NotifyResult, SeriesRef, required, text};

/// `DiscordColors`.
pub mod colors {
    pub const DANGER: i64 = 15_749_200;
    pub const SUCCESS: i64 = 2_605_644;
    pub const WARNING: i64 = 16_753_920;
    pub const STANDARD: i64 = 16_761_392;
    pub const UPGRADE: i64 = 4_089_856;
}

/// `GetTitle`: like the message without the quality, backticks escaped, 256 characters at most.
fn title(series: &SeriesRef, episodes: &[crate::EpisodeRef]) -> String {
    let t = if episodes.is_empty() {
        series.title.clone()
    } else {
        let m = crate::episode_message(series, episodes, "");
        m.trim_end_matches(" []").to_string()
    }
    .replace('`', "\\`");
    if t.chars().count() > 256 {
        let cut: String = t.chars().take(253).collect();
        format!("{}...", cut.trim_end_matches('\\'))
    } else {
        t
    }
}

/// `GetLinksString`.
fn links(series: &SeriesRef) -> String {
    let mut l = vec![
        format!(
            "[The TVDB](https://thetvdb.com/?tab=series&id={})",
            series.tvdb_id
        ),
        format!(
            "[Trakt](https://trakt.tv/search/tvdb/{}?id_type=show)",
            series.tvdb_id
        ),
    ];
    if let Some(imdb) = series.imdb_id.as_deref().filter(|i| !i.is_empty()) {
        l.push(format!("[IMDB](https://imdb.com/title/{imdb}/)"));
    }
    l.join(" / ")
}

/// `BytesToString`.
fn bytes(n: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n.max(0) as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, UNITS[i])
}

fn overview(o: Option<&str>) -> String {
    let o = o.unwrap_or_default();
    if o.chars().count() <= 300 {
        o.to_string()
    } else {
        format!("{}...", o.chars().take(300).collect::<String>())
    }
}

fn field(name: &str, value: impl Into<String>, inline: bool) -> Value {
    json!({"name": name, "value": value.into(), "inline": inline})
}

fn chosen(s: &Map<String, Value>, key: &str, default: &[i64]) -> Vec<i64> {
    match s.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_i64).collect(),
        _ => default.to_vec(),
    }
}

/// The embed for an event.
pub fn embed(s: &Map<String, Value>, event: &Event, instance: &Instance) -> Value {
    let author = json!({
        "name": text(s, "author").unwrap_or_else(|| instance.name.clone()),
    });
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut fields = Vec::new();
    let mut thumbnail = None;
    let mut image = None;
    let (title, description, color, url) = match event {
        Event::Test => (
            "Test Title".to_string(),
            "This is a test message from Kitsunarr".to_string(),
            colors::STANDARD,
            None,
        ),
        Event::Grab(g) => {
            let series = &g.series;
            // Overview, Rating, Genres, Quality, Group, Size, Links, Release, Poster, Fanart,
            // Indexer, CustomFormats, CustomFormatScore.
            for f in chosen(s, "grabFields", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]) {
                match f {
                    0 => fields.push(field(
                        "Overview",
                        overview(g.episodes.first().and_then(|e| e.overview.as_deref())),
                        false,
                    )),
                    1 => fields.push(field(
                        "Rating",
                        series.rating.map(|r| r.to_string()).unwrap_or_default(),
                        true,
                    )),
                    2 => fields.push(field(
                        "Genres",
                        series
                            .genres
                            .iter()
                            .take(5)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                        true,
                    )),
                    3 => fields.push(field(
                        "Quality",
                        crate::quality_string(
                            series,
                            &g.release.quality,
                            g.release.quality_version,
                        ),
                        true,
                    )),
                    4 => {
                        if let Some(group) = &g.release.release_group {
                            fields.push(field("Group", group.clone(), true));
                        }
                    }
                    5 => fields.push(field("Size", bytes(g.release.size), true)),
                    6 => fields.push(field("Links", links(series), false)),
                    7 => fields.push(field(
                        "Release",
                        format!("```{}```", g.release.release_title),
                        false,
                    )),
                    8 => thumbnail = series.poster().map(str::to_string),
                    9 => image = series.fanart().map(str::to_string),
                    10 => fields.push(field("Indexer", g.release.indexer.clone(), true)),
                    11 => fields.push(field(
                        "Custom Formats",
                        g.release.custom_formats.join("|"),
                        true,
                    )),
                    12 => fields.push(field(
                        "Custom Format Score",
                        g.release.custom_format_score.to_string(),
                        true,
                    )),
                    _ => {}
                }
            }
            (
                title(series, &g.episodes),
                "Episode Grabbed".to_string(),
                colors::STANDARD,
                Some(series),
            )
        }
        Event::Download(d) => {
            let series = &d.series;
            let upgrade = !d.deleted_files.is_empty();
            // Overview, Rating, Genres, Quality, Codecs, Group, Size, Languages, Subtitles,
            // Links, Release, Poster, Fanart, CustomFormats, CustomFormatScore.
            for f in chosen(s, "importFields", &[0, 1, 2, 3, 5, 6, 9, 10, 11, 12]) {
                match f {
                    0 => fields.push(field(
                        "Overview",
                        overview(d.episodes.first().and_then(|e| e.overview.as_deref())),
                        false,
                    )),
                    1 => fields.push(field(
                        "Rating",
                        series.rating.map(|r| r.to_string()).unwrap_or_default(),
                        true,
                    )),
                    2 => fields.push(field(
                        "Genres",
                        series
                            .genres
                            .iter()
                            .take(5)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                        true,
                    )),
                    3 => fields.push(field(
                        "Quality",
                        crate::quality_string(series, &d.file.quality, d.file.quality_version),
                        true,
                    )),
                    5 => {
                        if let Some(group) = &d.file.release_group {
                            fields.push(field("Group", group.clone(), true));
                        }
                    }
                    6 => fields.push(field("Size", bytes(d.file.size), true)),
                    7 => fields.push(field(
                        "Languages",
                        d.file
                            .languages
                            .iter()
                            .map(|l| l.name.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                        true,
                    )),
                    9 => fields.push(field("Links", links(series), false)),
                    10 => fields.push(field(
                        "Release",
                        format!(
                            "```{}```",
                            d.file
                                .scene_name
                                .clone()
                                .unwrap_or_else(|| d.file.relative_path.clone())
                        ),
                        false,
                    )),
                    11 => thumbnail = series.poster().map(str::to_string),
                    12 => image = series.fanart().map(str::to_string),
                    _ => {}
                }
            }
            (
                title(series, &d.episodes),
                if upgrade {
                    "Episode Upgraded"
                } else {
                    "Episode Imported"
                }
                .to_string(),
                if upgrade {
                    colors::UPGRADE
                } else {
                    colors::SUCCESS
                },
                Some(series),
            )
        }
        Event::Rename { series, files } => {
            thumbnail = series.poster().map(str::to_string);
            (
                series.title.clone(),
                format!("{} files renamed", files.len()),
                colors::STANDARD,
                Some(series.as_ref()),
            )
        }
        Event::SeriesAdd(series) => {
            thumbnail = series.poster().map(str::to_string);
            (
                series.title.clone(),
                "Series Added".to_string(),
                colors::SUCCESS,
                Some(series.as_ref()),
            )
        }
        Event::SeriesDelete {
            series,
            deleted_files,
        } => (
            series.title.clone(),
            if *deleted_files {
                "Series Deleted, along with its files"
            } else {
                "Series Deleted"
            }
            .to_string(),
            colors::DANGER,
            Some(series.as_ref()),
        ),
        Event::EpisodeFileDelete {
            series,
            episodes,
            reason,
            ..
        } => {
            fields.push(field("Reason", reason.clone(), false));
            (
                title(series, episodes),
                "Episode Deleted".to_string(),
                colors::DANGER,
                Some(series.as_ref()),
            )
        }
        Event::HealthIssue(h) => (
            h.source.clone(),
            h.message.clone(),
            if h.level == "warning" {
                colors::WARNING
            } else {
                colors::DANGER
            },
            None,
        ),
        Event::HealthRestored(h) => (
            format!("Health Issue Resolved: {}", h.source),
            format!("The following issue is now resolved: {}", h.message),
            colors::SUCCESS,
            None,
        ),
        Event::ManualInteractionRequired(m) => {
            thumbnail = m
                .series
                .as_ref()
                .and_then(|s| s.poster())
                .map(str::to_string);
            fields.push(field("Download", m.title.clone(), false));
            if !m.messages.is_empty() {
                fields.push(field("Reason", m.messages.join("\n"), false));
            }
            (
                m.series
                    .as_ref()
                    .map_or_else(|| m.title.clone(), |s| title(s, &m.episodes)),
                "Manual Interaction Required".to_string(),
                colors::WARNING,
                m.series.as_ref(),
            )
        }
        Event::ApplicationUpdate {
            previous_version,
            new_version,
        } => {
            fields.push(field("Previous Version", previous_version.clone(), true));
            fields.push(field("New Version", new_version.clone(), true));
            (
                "Application Updated".to_string(),
                event.message(),
                colors::STANDARD,
                None,
            )
        }
    };
    let mut e = json!({
        "author": author,
        "title": title,
        "description": description,
        "color": color,
        "fields": fields,
        "timestamp": now,
    });
    if let Some(series) = url.filter(|s| s.tvdb_id > 0) {
        e["url"] = format!("http://thetvdb.com/?tab=series&id={}", series.tvdb_id).into();
    }
    if let Some(t) = thumbnail {
        e["thumbnail"] = json!({"url": t});
    }
    if let Some(i) = image {
        e["image"] = json!({"url": i});
    }
    e
}

/// `DiscordProxy.SendPayload`.
pub async fn send(
    n: &Notifier,
    s: &Map<String, Value>,
    event: &Event,
    instance: &Instance,
) -> NotifyResult {
    let url = required(s, "webHookUrl", "The webhook URL")?;
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(NotifyError::Settings(
            "the webhook URL must start with https://".into(),
        ));
    }
    let mut payload = json!({ "embeds": [embed(s, event, instance)] });
    if let Some(u) = text(s, "username") {
        payload["username"] = u.into();
    }
    if let Some(a) = text(s, "avatar") {
        payload["avatar_url"] = a.into();
    }
    n.post_json(n.http.post(url), &payload).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::grab;

    #[test]
    fn grab_embeds_follow_sonarr() {
        let s = json!({"grabFields": [3, 4, 5, 7, 8]});
        let e = embed(
            s.as_object().unwrap(),
            &grab(),
            &Instance {
                name: "Kitsunarr".into(),
                application_url: None,
            },
        );
        assert_eq!(
            e["title"],
            "Breaking Bad - 1x01x02 - Pilot + Cat's in the Bag..."
        );
        assert_eq!(e["description"], "Episode Grabbed");
        assert_eq!(e["color"], colors::STANDARD);
        assert_eq!(e["author"]["name"], "Kitsunarr");
        let names: Vec<&str> = e["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Quality", "Group", "Size", "Release"]);
        assert_eq!(e["fields"][0]["value"], "HDTV-720p Proper");
        assert_eq!(e["fields"][2]["value"], "1.4 GB");
        assert_eq!(e["thumbnail"]["url"], "https://img/poster.jpg");
        assert_eq!(e["url"], "http://thetvdb.com/?tab=series&id=81189");
    }

    #[test]
    fn long_titles_are_cut() {
        let mut s = crate::tests::series();
        s.title = "x".repeat(300);
        assert_eq!(title(&s, &[]).chars().count(), 256);
        assert_eq!(bytes(1024), "1.0 KB");
    }
}
