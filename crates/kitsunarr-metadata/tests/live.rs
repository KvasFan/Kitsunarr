//! Tests against the real TVmaze and AniList services (network required).
//! `cargo test -p kitsunarr-metadata --test live -- --ignored`

use kitsunarr_domain::MetadataProvider;
use kitsunarr_metadata::{EntryKind, Metadata};

fn metadata() -> Metadata {
    Metadata::new(
        reqwest::Client::builder()
            .user_agent("Kitsunarr-tests/0.1 (+https://github.com/KvasFan/Kitsunarr)")
            .build()
            .unwrap(),
    )
}

#[tokio::test]
#[ignore = "real service: api.tvmaze.com"]
async fn tvmaze_search_and_details() {
    let m = metadata();
    let hits = m
        .search(MetadataProvider::TvMaze, "breaking bad")
        .await
        .unwrap();
    let hit = hits
        .iter()
        .find(|h| h.title == "Breaking Bad")
        .expect("Breaking Bad in results");
    let info = m.series(MetadataProvider::TvMaze, &hit.id).await.unwrap();
    assert_eq!(info.id, "169");
    assert!(info.episodes.len() >= 62);
    assert!(info.external_ids.iter().any(|e| e.value == "81189"));
    assert!(
        info.episodes.iter().any(|e| e.season_number == 0
            && e.episode_number == 1
            && e.title.as_deref() == Some("El Camino: A Breaking Bad Movie")),
        "unnumbered specials are fetched and placed in season 0"
    );
}

#[tokio::test]
#[ignore = "real service: graphql.anilist.co"]
async fn anilist_search_details_and_movies() {
    let m = metadata();
    let hits = m
        .search(MetadataProvider::AniList, "Cowboy Bebop")
        .await
        .unwrap();
    assert!(
        hits.iter()
            .any(|h| h.id == "1" && h.kind == EntryKind::Anime)
    );
    assert!(
        hits.iter()
            .any(|h| h.id == "5" && h.kind == EntryKind::Movie),
        "the film is found too"
    );
    let info = m.series(MetadataProvider::AniList, "1").await.unwrap();
    assert_eq!(info.episodes.len(), 26);
    assert!(
        info.alternate_titles
            .iter()
            .any(|t| t == "カウボーイビバップ")
    );
}

#[tokio::test]
#[ignore = "real service: skyhook.sonarr.tv"]
async fn thetvdb_through_skyhook() {
    let m = metadata();
    let hits = m
        .search(MetadataProvider::Tvdb, "breaking bad")
        .await
        .unwrap();
    assert!(hits.iter().any(|h| h.id == "81189"));
    let info = m.series(MetadataProvider::Tvdb, "81189").await.unwrap();
    assert_eq!(info.title, "Breaking Bad");
    assert!(info.episodes.len() >= 62);
    let dvd = m
        .series_ordered(MetadataProvider::Tvdb, "76885", Some("dvd"))
        .await
        .unwrap();
    assert_eq!(dvd.kind, EntryKind::Anime);
    assert!(!dvd.episodes.is_empty());
}

#[tokio::test]
#[ignore = "real service: api.jikan.moe"]
async fn myanimelist_through_jikan() {
    let m = metadata();
    let hits = m
        .search(MetadataProvider::MyAnimeList, "cowboy bebop")
        .await
        .unwrap();
    assert!(hits.iter().any(|h| h.id == "1"));
    let info = m.series(MetadataProvider::MyAnimeList, "1").await.unwrap();
    assert_eq!(info.episodes.len(), 26);
    assert!(info.episodes[0].title.is_some());
}
