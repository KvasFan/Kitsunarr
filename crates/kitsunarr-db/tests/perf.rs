//! Large-library query benchmark (ignored by default).
//!
//! `cargo test -p kitsunarr-db --release --test perf -- --ignored --nocapture`
//!
//! Seeds 1,000 series (750 TV, 250 anime), 25,000 episodes, 12,000 episode files and 5,000
//! history rows into a file database, then reports the median time of the queries the web
//! interface issues. Absolute numbers depend on the machine; the assertions only guard against
//! pathological regressions.

use std::time::{Duration, Instant};

use kitsunarr_db::library::series::{self, NewEpisode, NewEpisodeFile, NewSeason, NewSeries};
use kitsunarr_db::library::stats;
use kitsunarr_db::{Database, history};
use kitsunarr_domain::api::library::LibraryQuery;
use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MediaKind, MetadataProvider, MonitorNewItems,
    PageRequest, SeriesStatus, SeriesType, SortDirection,
};

const SERIES: i64 = 1_000;
const EPISODES_PER_SERIES: i32 = 25;
const FILES_PER_SERIES: i32 = 12;
const HISTORY: i64 = 5_000;

async fn seed(db: &Database) -> i64 {
    let mut tx = db.pool().begin().await.unwrap();
    let mut first = 0;
    for n in 0..SERIES {
        let anime = n % 4 == 0;
        let title = format!("{} Series {n:04}", if anime { "Anime" } else { "Drama" });
        let id = series::insert(
            &mut tx,
            &NewSeries {
                media_kind: if anime {
                    MediaKind::Anime
                } else {
                    MediaKind::Series
                },
                title: title.clone(),
                year: Some(1990 + (n % 35) as i32),
                status: if n % 3 == 0 {
                    SeriesStatus::Continuing
                } else {
                    SeriesStatus::Ended
                },
                overview: Some("Synthetic benchmark series.".into()),
                network: Some(format!("Network {}", n % 40)),
                air_time: None,
                runtime: Some(45),
                certification: None,
                original_language: Some(1),
                genres: vec!["Drama".into()],
                images: vec![MediaImage {
                    cover_type: "poster".into(),
                    remote_url: format!("https://img.example/{n}.jpg"),
                }],
                ratings: None,
                first_aired: Some("2000-01-01".into()),
                last_aired: None,
                series_type: if anime {
                    SeriesType::Anime
                } else {
                    SeriesType::Standard
                },
                metadata_provider: MetadataProvider::Tvdb,
                metadata_id: (100_000 + n).to_string(),
                episode_ordering: None,
                monitored: n % 5 != 0,
                monitor_new_items: MonitorNewItems::All,
                season_folder: true,
                use_scene_numbering: false,
                quality_profile_id: 1,
                root_folder_path: "/media/tv".into(),
                path: format!("/media/tv/{title}"),
                added_at: format!("2026-0{}-01T00:00:00Z", 1 + n % 9),
                last_refreshed_at: None,
                tags: vec![],
                external_ids: vec![ExternalId {
                    kind: ExternalIdKind::Tvdb,
                    value: (100_000 + n).to_string(),
                }],
                alternate_titles: vec![(format!("Alias {n}"), None, None)],
            },
        )
        .await
        .unwrap();
        if n == 0 {
            first = id;
        }
        for season in 1..=2 {
            series::upsert_season(
                &mut tx,
                id,
                &NewSeason {
                    season_number: season,
                    monitored: true,
                    title: None,
                    images: vec![],
                },
            )
            .await
            .unwrap();
        }
        for e in 1..=EPISODES_PER_SERIES {
            let season = if e <= 13 { 1 } else { 2 };
            let file = if e <= FILES_PER_SERIES {
                Some(
                    series::upsert_episode_file(
                        &mut tx,
                        &NewEpisodeFile {
                            series_id: id,
                            season_number: season,
                            relative_path: format!("Season {season:02}/E{e:02}.mkv"),
                            size: 1_500_000_000,
                            added_at: "2026-01-01T00:00:00Z".into(),
                            scene_name: None,
                            release_group: Some("GRP".into()),
                            quality_id: 7,
                            revision_version: 1,
                            revision_real: 0,
                            is_repack: false,
                            languages: vec![1],
                            media_info: None,
                            original_path: None,
                            indexer_flags: 0,
                            release_type: 1,
                        },
                    )
                    .await
                    .unwrap(),
                )
            } else {
                None
            };
            // Spread air dates from the past into the next weeks so calendar queries return data.
            let days = (n * 7 + i64::from(e) * 3) % 400 - 200;
            let air = (chrono::Utc::now() + chrono::Duration::days(days))
                .format("%Y-%m-%dT20:00:00Z")
                .to_string();
            series::upsert_episode(
                &mut tx,
                &NewEpisode {
                    series_id: id,
                    season_number: season,
                    episode_number: e,
                    absolute_episode_number: Some(e),
                    scene_season_number: None,
                    scene_episode_number: None,
                    scene_absolute_episode_number: None,
                    unverified_scene_numbering: false,
                    title: Some(format!("Episode {e}")),
                    overview: None,
                    air_date: Some(air[..10].to_string()),
                    air_date_utc: Some(air),
                    runtime: Some(45),
                    monitored: true,
                    episode_file_id: file,
                    provider_episode_id: None,
                    last_search_at: None,
                },
            )
            .await
            .unwrap();
        }
    }
    for h in 0..HISTORY {
        history::insert(
            &mut tx,
            &history::NewHistory {
                event_type: if h % 2 == 0 {
                    "grabbed".into()
                } else {
                    "downloadFolderImported".into()
                },
                date: format!("2026-{:02}-{:02}T12:00:00Z", 1 + h % 10, 1 + h % 28),
                source_title: format!("Release.{h}.1080p.WEB-DL-GRP"),
                series_id: Some(first + h % SERIES),
                episode_id: None,
                movie_id: None,
                quality_id: Some(3),
                revision: None,
                languages: vec![1],
                custom_format_score: None,
                download_id: Some(format!("hash{h}")),
                data: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    first
}

async fn median<F, Fut>(runs: usize, mut f: F) -> Duration
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        f().await;
        times.push(start.elapsed());
    }
    times.sort();
    times[runs / 2]
}

#[tokio::test]
#[ignore = "benchmark; run explicitly in release mode"]
async fn large_library_queries() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("perf.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let start = Instant::now();
    let first = seed(&db).await;
    let seed_time = start.elapsed();
    let episodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episode_files")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        (episodes, files),
        (
            SERIES * i64::from(EPISODES_PER_SERIES),
            SERIES * i64::from(FILES_PER_SERIES)
        )
    );

    let q = |sort: &str, desc: bool, search: Option<&str>, kind: Option<MediaKind>| LibraryQuery {
        page: Some(1),
        page_size: Some(60),
        sort_key: Some(sort.into()),
        sort_direction: Some(if desc {
            SortDirection::Desc
        } else {
            SortDirection::Asc
        }),
        search: search.map(Into::into),
        kind,
        ..Default::default()
    };
    let now = chrono::Utc::now();
    let fmt =
        |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (cal_start, cal_end) = (
        fmt(now - chrono::Duration::days(1)),
        fmt(now + chrono::Duration::days(14)),
    );
    let runs = 15;
    let mut rows: Vec<(&str, Duration)> = Vec::new();
    rows.push((
        "series page 1, sort by title",
        median(runs, || async {
            series::list(&db, &q("title", false, None, None))
                .await
                .unwrap();
        })
        .await,
    ));
    rows.push((
        "series page 10, sort by title",
        median(runs, || async {
            let mut query = q("title", false, None, None);
            query.page = Some(10);
            series::list(&db, &query).await.unwrap();
        })
        .await,
    ));
    rows.push((
        "series page 1, sort by size",
        median(runs, || async {
            series::list(&db, &q("size", true, None, None))
                .await
                .unwrap();
        })
        .await,
    ));
    rows.push((
        "series page 1, sort by next airing",
        median(runs, || async {
            series::list(&db, &q("nextAiring", false, None, None))
                .await
                .unwrap();
        })
        .await,
    ));
    rows.push((
        "anime only",
        median(runs, || async {
            series::list(&db, &q("title", false, None, Some(MediaKind::Anime)))
                .await
                .unwrap();
        })
        .await,
    ));
    rows.push((
        "title search",
        median(runs, || async {
            series::list(&db, &q("title", false, Some("series 07"), None))
                .await
                .unwrap();
        })
        .await,
    ));
    rows.push((
        "dashboard statistics",
        median(runs, || async {
            stats::library_stats(&db).await.unwrap();
        })
        .await,
    ));
    rows.push((
        "calendar, 15 days",
        median(runs, || async {
            series::calendar(&db, &cal_start, &cal_end, false)
                .await
                .unwrap();
        })
        .await,
    ));
    rows.push((
        "series detail",
        median(runs, || async {
            series::get_detail(&db, first).await.unwrap();
        })
        .await,
    ));
    rows.push((
        "episodes of one series",
        median(runs, || async {
            series::list_episodes(&db, first, None).await.unwrap();
        })
        .await,
    ));
    rows.push((
        "history page 1",
        median(runs, || async {
            history::list(&db, &PageRequest::default(), None)
                .await
                .unwrap();
        })
        .await,
    ));

    println!(
        "\nSeeded {SERIES} series / {episodes} episodes / {files} files / {HISTORY} history rows in {:.1?}",
        seed_time
    );
    println!("| Query | Median ({runs} runs) |\n|---|---:|");
    for (name, t) in &rows {
        println!("| {name} | {:.2} ms |", t.as_secs_f64() * 1000.0);
    }
    for (name, t) in rows {
        assert!(t < Duration::from_secs(2), "{name} took {t:?}");
    }
}
