//! Tanukarr's `SeriesDetailsLinks`: links to the title on its catalogues, with its ids to copy.

use dioxus::prelude::*;
use kitsunarr_domain::{ExternalId, ExternalIdKind, MetadataProvider};

use crate::state::use_app;

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalLink {
    pub name: &'static str,
    pub url: String,
    /// The id shown for copying, when the link has one.
    pub id: Option<String>,
}

fn positive(v: &str) -> Option<String> {
    let v = v.trim();
    (v.parse::<u64>().is_ok_and(|n| n > 0)).then(|| v.to_string())
}

fn imdb(v: &str) -> Option<String> {
    let v = v.trim();
    (v.starts_with("tt") && v.len() > 2 && v[2..].chars().all(|c| c.is_ascii_digit()))
        .then(|| v.to_string())
}

/// The links of a title: its own catalogue first (Tanukarr's `getMetadataProvider`), then
/// TheTVDB, TVmaze, IMDb (with Trakt and MDBList), TMDB, AniList and MyAnimeList; links with
/// an id come first.
pub fn links(
    provider: MetadataProvider,
    metadata_id: &str,
    ids: &[ExternalId],
    movie: bool,
) -> Vec<ExternalLink> {
    let first = |kind: ExternalIdKind, check: fn(&str) -> Option<String>| {
        ids.iter()
            .filter(|x| x.kind == kind)
            .find_map(|x| check(&x.value))
    };
    let tmdb_path = if movie { "movie" } else { "tv" };
    let mut out: Vec<ExternalLink> = Vec::new();
    let mut push = |name: &'static str, url: String, id: Option<String>| {
        if !out.iter().any(|l| l.name == name) {
            out.push(ExternalLink { name, url, id });
        }
    };
    let own = match provider {
        MetadataProvider::AniList => positive(metadata_id)
            .map(|id| ("AniList", format!("https://anilist.co/anime/{id}"), id)),
        MetadataProvider::MyAnimeList => positive(metadata_id).map(|id| {
            (
                "MyAnimeList",
                format!("https://myanimelist.net/anime/{id}"),
                id,
            )
        }),
        MetadataProvider::TvMaze => positive(metadata_id)
            .map(|id| ("TVmaze", format!("https://www.tvmaze.com/shows/{id}"), id)),
        MetadataProvider::TmdbTv | MetadataProvider::TmdbMovie => positive(metadata_id)
            .or_else(|| first(ExternalIdKind::Tmdb, positive))
            .map(|id| {
                (
                    "TMDB",
                    format!("https://www.themoviedb.org/{tmdb_path}/{id}"),
                    id,
                )
            }),
        MetadataProvider::Omdb => imdb(metadata_id)
            .or_else(|| first(ExternalIdKind::Imdb, imdb))
            .map(|id| ("IMDb", format!("https://www.imdb.com/title/{id}/"), id)),
        _ => None,
    };
    if let Some((name, url, id)) = own {
        push(name, url, Some(id));
    }
    let tvdb = first(ExternalIdKind::Tvdb, positive).or_else(|| {
        (provider == MetadataProvider::Tvdb)
            .then(|| positive(metadata_id))
            .flatten()
    });
    if let Some(id) = tvdb {
        push(
            "TheTVDB",
            format!("https://www.thetvdb.com/?tab=series&id={id}"),
            Some(id),
        );
    }
    if let Some(id) = first(ExternalIdKind::TvMaze, positive) {
        push(
            "TVmaze",
            format!("https://www.tvmaze.com/shows/{id}/_"),
            Some(id),
        );
    }
    if let Some(id) = first(ExternalIdKind::Imdb, imdb) {
        let kind = if movie { "movie" } else { "show" };
        push(
            "IMDb",
            format!("https://imdb.com/title/{id}/"),
            Some(id.clone()),
        );
        push(
            "Trakt",
            format!(
                "https://trakt.tv/{}s/{id}",
                if movie { "movie" } else { "show" }
            ),
            None,
        );
        push("MDBList", format!("https://mdblist.com/{kind}/{id}"), None);
    }
    if let Some(id) = first(ExternalIdKind::Tmdb, positive) {
        push(
            "TMDB",
            format!("https://www.themoviedb.org/{tmdb_path}/{id}"),
            Some(id),
        );
    }
    if let Some(id) = first(ExternalIdKind::AniList, positive) {
        push(
            "AniList",
            format!("https://anilist.co/anime/{id}"),
            Some(id),
        );
    }
    if let Some(id) = first(ExternalIdKind::MyAnimeList, positive) {
        push(
            "MyAnimeList",
            format!("https://myanimelist.net/anime/{id}"),
            Some(id),
        );
    }
    out.sort_by_key(|l| l.id.is_none());
    out
}

#[component]
pub fn ExternalLinks(
    provider: MetadataProvider,
    metadata_id: String,
    external_ids: Vec<ExternalId>,
    #[props(default)] movie: bool,
) -> Element {
    let app = use_app();
    let list = links(provider, &metadata_id, &external_ids, movie);
    if list.is_empty() {
        return rsx! { span { class: "muted", "—" } };
    }
    rsx! {
        span { class: "external-links", role: "list", "aria-label": "Links",
            for l in list {
                span { key: "{l.name}", class: "external-link", role: "listitem",
                    a { href: "{l.url}", target: "_blank", rel: "noopener noreferrer", "{l.name}" }
                    if let Some(id) = l.id.clone() {
                        button { class: "btn ghost icon xs", r#type: "button", title: "Copy {id}", "aria-label": "Copy the {l.name} id",
                            onclick: move |_| { crate::util::copy_to_clipboard(&id); app.success("Copied."); },
                            "⧉"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(kind: ExternalIdKind, value: &str) -> ExternalId {
        ExternalId {
            kind,
            value: value.into(),
        }
    }

    #[test]
    fn series_and_movie_links_follow_tanukarr() {
        let l = links(
            MetadataProvider::AniList,
            "21",
            &[
                id(ExternalIdKind::Tvdb, "81797"),
                id(ExternalIdKind::Imdb, "tt0388629"),
                id(ExternalIdKind::MyAnimeList, "21"),
            ],
            false,
        );
        let names: Vec<_> = l.iter().map(|l| l.name).collect();
        assert_eq!(
            names,
            [
                "AniList",
                "TheTVDB",
                "IMDb",
                "MyAnimeList",
                "Trakt",
                "MDBList"
            ]
        );
        assert_eq!(l[0].url, "https://anilist.co/anime/21");
        assert_eq!(l[4].url, "https://trakt.tv/shows/tt0388629");
        let m = links(MetadataProvider::TmdbMovie, "603", &[], true);
        assert_eq!(m[0].url, "https://www.themoviedb.org/movie/603");
        assert!(links(MetadataProvider::TmdbTv, "0", &[], false).is_empty());
    }
}
