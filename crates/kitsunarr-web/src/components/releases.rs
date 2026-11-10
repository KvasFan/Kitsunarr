//! Interactive release search results.

use dioxus::prelude::*;
use kitsunarr_domain::DownloadProtocol;
use kitsunarr_domain::api::search::{ReleaseItem, ReleaseQuery};

use crate::components::common::{Empty, ErrorBox, Loading};
use crate::route::Route;
use crate::state::use_client;
use crate::util::{format_bytes, relative};

fn episodes_label(r: &ReleaseItem) -> String {
    if !r.parsed {
        return "unparsed".into();
    }
    if r.full_season {
        return match r.season_numbers.as_slice() {
            [one] => format!("Season {one}"),
            [first, .., last] => format!("Seasons {first}-{last}"),
            [] => "Full season".into(),
        };
    }
    if !r.episode_numbers.is_empty() {
        let season = r.season_numbers.first().copied().unwrap_or(0);
        let eps: Vec<String> = r
            .episode_numbers
            .iter()
            .map(|e| format!("E{e:02}"))
            .collect();
        return format!("S{season:02}{}", eps.join(""));
    }
    if !r.absolute_episode_numbers.is_empty() {
        let abs: Vec<String> = r
            .absolute_episode_numbers
            .iter()
            .map(|a| a.to_string())
            .collect();
        return abs.join("-");
    }
    if r.special {
        "Special".into()
    } else {
        "—".into()
    }
}

/// Searches the indexers for a season or an episode of a series and lists the releases.
#[component]
pub fn ReleaseSearch(
    series_id: i64,
    #[props(default)] season: Option<i32>,
    #[props(default)] episode_id: Option<i64>,
) -> Element {
    let client = use_client();
    let results = use_resource(move || {
        let c = client.clone();
        let q = ReleaseQuery {
            series_id,
            season_number: season,
            episode_id,
        };
        async move { c.search_releases(&q).await }
    });
    let read = results.read();
    let Some(outcome) = &*read else {
        return rsx! { Loading { label: "Searching indexers…" } };
    };
    let r = match outcome {
        Ok(r) => r.clone(),
        Err(e) => return rsx! { ErrorBox { message: e.to_string() } },
    };
    drop(read);
    rsx! {
        p { class: "small muted",
            "{r.description}: {r.releases.len()} releases from {r.searched_indexers} indexer(s)."
        }
        for f in r.failures.iter() {
            div { key: "{f.indexer_id}", class: "alert warning", role: "status",
                div { class: "grow", strong { "{f.indexer}: " } "{f.message}" }
            }
        }
        for (i, w) in r.warnings.iter().enumerate() {
            div { key: "w{i}", class: "alert warning", role: "status", div { class: "grow", "{w}" } }
        }
        if r.searched_indexers == 0 {
            Empty { title: "No indexers to search",
                p { "Add a Newznab or Torznab indexer and enable interactive search for it." }
                Link { to: Route::SettingsIndexers {}, class: "btn", "Indexer settings" }
            }
        } else if r.releases.is_empty() {
            Empty { title: "Nothing found",
                p { "The indexers returned no releases for this search." }
            }
        } else {
            p { class: "help small faint",
                "Releases are checked against the series' quality profile, custom formats and release profiles, and ranked, as Sonarr does. Grabbing needs download clients, which are the next milestone."
            }
            div { class: "table-wrap",
                table { class: "table releases",
                    thead {
                        tr {
                            th { span { class: "sr-only", "Decision" } }
                            th { "Source" }
                            th { "Age" }
                            th { "Title" }
                            th { "Episodes" }
                            th { "Indexer" }
                            th { class: "num", "Size" }
                            th { class: "num", "Peers" }
                            th { "Languages" }
                            th { "Quality" }
                            th { class: "num", "Score" }
                            th { span { class: "sr-only", "Actions" } }
                        }
                    }
                    tbody {
                        for rel in r.releases.iter() {
                            tr { key: "{rel.indexer_id}-{rel.guid}", class: if rel.approved { "" } else { "rejected" },
                                td {
                                    if rel.approved {
                                        span { class: "decision ok", title: "Accepted", "aria-label": "Accepted", "✓" }
                                    } else {
                                        span { class: "decision no", title: "{rel.rejections.join(\"\n\")}", "aria-label": "Rejected", "✕" }
                                    }
                                }
                                td {
                                    span { class: if rel.protocol == DownloadProtocol::Torrent { "badge info" } else { "badge accent" },
                                        if rel.protocol == DownloadProtocol::Torrent { "torrent" } else { "nzb" }
                                    }
                                }
                                td { class: "small age", title: "{rel.publish_date}", {relative(&rel.publish_date)} }
                                td { class: "release-title",
                                    span { class: "mono small", "{rel.title}" }
                                    for flag in rel.indexer_flags.iter() {
                                        span { key: "{flag}", class: "badge", " {flag}" }
                                    }
                                    for cf in rel.custom_formats.iter() {
                                        span { key: "cf-{cf}", class: "badge accent", "{cf}" }
                                    }
                                    if !rel.rejections.is_empty() {
                                        ul { class: "rejections small",
                                            for (i, why) in rel.rejections.iter().enumerate() {
                                                li { key: "{i}", "{why}" }
                                            }
                                        }
                                    }
                                }
                                td { class: "small", title: "{rel.mapped_episodes.join(\", \")}", {episodes_label(rel)} }
                                td { class: "small", "{rel.indexer}" }
                                td { class: "num small", {format_bytes(rel.size)} }
                                td { class: "num small",
                                    {match (rel.seeders, rel.peers) {
                                        (Some(s), Some(p)) => format!("{s} / {p}"),
                                        (Some(s), None) => s.to_string(),
                                        _ => "—".into(),
                                    }}
                                }
                                td { class: "small", {rel.languages.join(", ")} }
                                td { span { class: "badge", "{rel.quality}" } }
                                td { class: "num small", title: "Custom format score", "{rel.custom_format_score}" }
                                td { class: "actions",
                                    div { class: "row tight",
                                        if let Some(url) = rel.info_url.as_ref() {
                                            a { class: "btn sm ghost", href: "{url}", target: "_blank", rel: "noopener noreferrer", "aria-label": "Open on {rel.indexer}", "↗" }
                                        }
                                        button { class: "btn sm", disabled: true, title: "Download clients are not implemented yet", "Grab" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
