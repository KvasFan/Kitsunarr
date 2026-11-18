//! HTTP tests of the endpoints behind Tanukarr's interface pages: naming samples, statistics,
//! collection settings, Settings › UI and host, log files, the parse tester and manual
//! grouping when adding.

use std::net::SocketAddr;
use std::sync::Arc;

use kitsunarr_api::{AppState, BuildInfo, LogBuffer, NoAssets, NoTasks};
use kitsunarr_client::{Client, ClientError};
use kitsunarr_config::Config;
use kitsunarr_db::Database;
use kitsunarr_db::sqlx;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_domain::api::auth::SetupRequest;
use kitsunarr_domain::api::library::{CollectionEdit, CollectionMemberEdit};
use kitsunarr_domain::api::metadata::{AddTitle, AddTitleRequest, MonitorOption};
use kitsunarr_domain::api::settings::{NamingSettings, RootFolderCreate};
use kitsunarr_domain::api::views::{CollectionSettingsUpdate, StatisticsQuery};
use kitsunarr_metadata::Metadata;

struct Server {
    base: String,
    api_key: String,
    state: AppState,
    dir: tempfile::TempDir,
}

async fn start_with(metadata: Metadata) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let api_key = kitsunarr_db::seed::ensure_api_key(&db, kitsunarr_auth::generate_api_key)
        .await
        .unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        config_dir,
        ..Default::default()
    };
    // As if the server had been started with `KITSUNARR_URL_BASE`.
    config.mark_overridden("server.url_base");
    let state = AppState::new(
        db,
        Arc::new(config),
        kitsunarr_events::EventBus::default(),
        api_key.clone(),
        BuildInfo {
            version: "test",
            git_revision: None,
        },
        Arc::new(NoAssets),
        Arc::new(NoTasks),
        reqwest::Client::new(),
        metadata,
        LogBuffer::default(),
    );
    let app = kitsunarr_api::app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server {
        base: format!("http://{addr}"),
        api_key,
        state,
        dir,
    }
}

async fn start() -> Server {
    start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: "http://127.0.0.1:9".to_string(),
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await
}

async fn signed_in(server: &Server) -> Client {
    let client = Client::new(&server.base);
    client
        .setup(&SetupRequest {
            username: "admin".into(),
            password: "correct horse battery".into(),
        })
        .await
        .unwrap();
    client
}

fn status(e: ClientError) -> u16 {
    e.status().unwrap_or(0)
}

async fn v3(
    server: &Server,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut req = reqwest::Client::new()
        .request(method, format!("{}/api/v3{path}", server.base))
        .header("X-Api-Key", &server.api_key);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.unwrap();
    assert!(resp.status().is_success(), "{path}: {}", resp.status());
    resp.json().await.unwrap()
}

async fn profile(pool: &sqlx::SqlitePool, nth: i64) -> i64 {
    sqlx::query_scalar("SELECT id FROM quality_profiles ORDER BY id LIMIT 1 OFFSET ?")
        .bind(nth)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_series(pool: &sqlx::SqlitePool, id: i64, title: &str, kind: &str, status: &str) {
    let profile = profile(pool, 0).await;
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, status, series_type, metadata_provider, \
         metadata_id, quality_profile_id, root_folder_path, path, added_at, genres, ratings) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'tvMaze', ?, ?, '/tv', ?, '2026-11-01T00:00:00Z', '[\"Drama\"]', '{\"value\": 8.5, \"votes\": 10}')",
    )
    .bind(id)
    .bind(if kind == "anime" { "anime" } else { "series" })
    .bind(title)
    .bind(title.to_lowercase())
    .bind(title.to_lowercase())
    .bind(format!("slug-{id}"))
    .bind(status)
    .bind(kind)
    .bind(id.to_string())
    .bind(profile)
    .bind(format!("/tv/{title}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO seasons (series_id, season_number) VALUES (?, 1)")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn naming_examples_follow_unsaved_settings() {
    let server = start().await;
    let client = signed_in(&server).await;
    let mut naming = NamingSettings {
        rename_episodes: true,
        standard_episode_format: "{Series Title} - S{season:00}E{episode:00}".into(),
        multi_episode_style: 4,
        series_folder_format: "{Series TitleYear}".into(),
        movie_folder_format: "{Movie Title} [{Release Year}]".into(),
        ..Default::default()
    };
    let e = client.naming_examples(&naming).await.unwrap();
    assert_eq!(
        e.single_episode_example.as_deref(),
        Some("The Series Title's! - S01E01")
    );
    // Range: S01E01-03.
    assert_eq!(
        e.multi_episode_example.as_deref(),
        Some("The Series Title's! - S01E01-03")
    );
    assert_eq!(
        e.series_folder_example.as_deref(),
        Some("The Series Title's! (2010)")
    );
    assert_eq!(
        e.movie_folder_example.as_deref(),
        Some("The Movie Title [2010]")
    );
    assert!(e.season_folder_example.is_some() && e.anime_group_folder_example.is_some());
    // The stored settings are untouched; an empty format has no example.
    naming.season_folder_format = String::new();
    let e = client.naming_examples(&naming).await.unwrap();
    assert_eq!(e.season_folder_example, None);
    assert_eq!(
        client.naming().await.unwrap().standard_episode_format,
        NamingSettings::default().standard_episode_format
    );
}

#[tokio::test]
async fn statistics_break_the_library_down() {
    let server = start().await;
    let client = signed_in(&server).await;
    let pool = server.state.db.pool();
    insert_series(pool, 1, "Alpha", "standard", "continuing").await;
    insert_series(pool, 2, "Beta", "anime", "ended").await;
    let tag = client.create_tag("favourite").await.unwrap();
    sqlx::query("INSERT INTO series_tags (series_id, tag_id) VALUES (2, ?)")
        .bind(tag.id)
        .execute(pool)
        .await
        .unwrap();
    // Beta: one episode with a file, one aired and missing, one not aired yet.
    sqlx::query("INSERT INTO episode_files (id, series_id, season_number, relative_path, size, added_at, quality_id) VALUES (1, 2, 1, 'a.mkv', 1000, '2026-11-01T00:00:00Z', 4)")
        .execute(pool)
        .await
        .unwrap();
    for (n, air, file) in [
        (1, "2020-01-01T00:00:00Z", Some(1)),
        (2, "2020-01-08T00:00:00Z", None),
        (3, "2999-01-01T00:00:00Z", None),
    ] {
        sqlx::query("INSERT INTO episodes (series_id, season_number, episode_number, air_date_utc, episode_file_id) VALUES (2, 1, ?, ?, ?)")
            .bind(n)
            .bind(air)
            .bind(file)
            .execute(pool)
            .await
            .unwrap();
    }
    let s = client
        .statistics(&StatisticsQuery::default())
        .await
        .unwrap();
    assert_eq!(s.series_count, 2);
    assert_eq!((s.continuing_series_count, s.ended_series_count), (1, 1));
    assert_eq!((s.standard_series_count, s.anime_series_count), (1, 1));
    assert_eq!(s.total_episode_count, 3);
    assert_eq!(s.downloaded_episode_count, 1);
    assert_eq!(s.missing_episode_count, 1);
    assert_eq!(s.unaired_episode_count, 1);
    assert_eq!((s.episode_file_count, s.size_on_disk), (1, 1000));
    assert_eq!(s.quality_statistics.len(), 1);
    assert_eq!(s.quality_statistics[0].quality.id, 4);
    assert!(!s.quality_statistics[0].quality.name.is_empty());
    let fav = s
        .tag_statistics
        .iter()
        .find(|t| t.label == "favourite")
        .unwrap();
    assert_eq!((fav.series_count, fav.episode_file_count), (1, 1));
    let first = s
        .quality_profile_statistics
        .iter()
        .find(|p| p.series_count == 2)
        .unwrap();
    assert_eq!(first.size_on_disk, 1000);
    // Filters narrow every count.
    let anime = client
        .statistics(&StatisticsQuery {
            series_type: Some(kitsunarr_domain::SeriesType::Anime),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!((anime.series_count, anime.total_episode_count), (1, 3));
    let tagged = client
        .statistics(&StatisticsQuery {
            tag_id: Some(tag.id + 100),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tagged.series_count, 0);
    // The library list carries the index columns.
    let page = client
        .series(&kitsunarr_domain::api::library::LibraryQuery::default())
        .await
        .unwrap();
    let beta = page.records.iter().find(|r| r.id == 2).unwrap();
    assert_eq!(beta.index.genres, vec!["Drama"]);
    assert_eq!(beta.index.rating, Some(8.5));
    assert_eq!(
        beta.index.previous_airing.as_deref(),
        Some("2020-01-08T00:00:00Z")
    );
}

#[tokio::test]
async fn collection_settings_apply_to_members() {
    let server = start().await;
    let client = signed_in(&server).await;
    let pool = server.state.db.pool();
    insert_series(pool, 1, "Part One", "anime", "ended").await;
    insert_series(pool, 2, "Part Two", "anime", "ended").await;
    let other_profile = profile(pool, 1).await;
    sqlx::query(
        "INSERT INTO movies (id, title, sort_title, clean_title, title_slug, metadata_provider, metadata_id, quality_profile_id, \
         root_folder_path, path, added_at) VALUES (5, 'The Film', 'film', 'film', 'film', 'tmdbMovie', '5', ?, '/m', '/m/film', '2026-11-01T00:00:00Z')",
    )
    .bind(profile(pool, 0).await)
    .execute(pool)
    .await
    .unwrap();
    let c = client
        .create_collection(&CollectionEdit {
            title: "Saga".into(),
            overview: None,
        })
        .await
        .unwrap();
    assert_eq!(
        (c.group_kind.as_str(), c.movie_folder_style.as_str()),
        ("series", "automatic")
    );
    for (series, movie) in [(Some(1), None), (Some(2), None), (None, Some(5))] {
        client
            .add_collection_member(
                c.id,
                &CollectionMemberEdit {
                    series_id: series,
                    movie_id: movie,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let d = client
        .update_collection_settings(
            c.id,
            &CollectionSettingsUpdate {
                title: Some("The Saga".into()),
                movie_folder_style: Some("title".into()),
                monitored: Some(false),
                quality_profile_id: Some(other_profile),
                season_folder: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(d.title, "The Saga");
    assert_eq!(d.movie_folder_style, "title");
    for id in [1, 2] {
        let s = client.series_detail(id).await.unwrap();
        assert!(!s.item.monitored && !s.season_folder);
        assert_eq!(s.item.quality_profile_id, other_profile);
        let g = s.group_membership.unwrap();
        assert_eq!(
            (g.title.as_str(), g.movie_folder_style.as_str()),
            ("The Saga", "title")
        );
    }
    let (monitored, movie_profile): (bool, i64) =
        sqlx::query_as("SELECT monitored, quality_profile_id FROM movies WHERE id = 5")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(!monitored);
    assert_eq!(movie_profile, other_profile);
    // A saga holds only films; unknown styles and profiles are refused.
    for bad in [
        CollectionSettingsUpdate {
            group_kind: Some("movieSaga".into()),
            ..Default::default()
        },
        CollectionSettingsUpdate {
            movie_folder_style: Some("fancy".into()),
            ..Default::default()
        },
        CollectionSettingsUpdate {
            quality_profile_id: Some(9999),
            ..Default::default()
        },
    ] {
        let e = client
            .update_collection_settings(c.id, &bad)
            .await
            .unwrap_err();
        assert!(matches!(status(e), 400 | 422));
    }
    assert_eq!(
        status(
            client
                .update_collection_settings(999, &CollectionSettingsUpdate::default())
                .await
                .unwrap_err()
        ),
        404
    );
    // Sonarr v3 shows the stored values, and its series editor changes them for whole groups.
    let v = v3(&server, reqwest::Method::GET, "/series/1", None).await;
    assert_eq!(v["animeGroupMovieFolderStyle"], "title");
    assert_eq!(v["animeGroupKind"], "series");
    v3(
        &server,
        reqwest::Method::PUT,
        "/series/editor",
        Some(serde_json::json!({"seriesIds": [1, 2], "animeGroupMovieFolderStyle": "numbered"})),
    )
    .await;
    assert_eq!(
        client.collection(c.id).await.unwrap().movie_folder_style,
        "numbered"
    );
    let partial = reqwest::Client::new()
        .put(format!("{}/api/v3/series/editor", server.base))
        .header("X-Api-Key", &server.api_key)
        .json(&serde_json::json!({"seriesIds": [1], "animeGroupKind": "series"}))
        .send()
        .await
        .unwrap();
    assert!(partial.status().is_client_error());
}

#[tokio::test]
async fn ui_settings_round_trip_natively_and_through_sonarr_apis() {
    let server = start().await;
    let client = signed_in(&server).await;
    let mut g = client.general().await.unwrap();
    assert_eq!(g.short_date_format, "MMM D YYYY");
    assert!(g.show_relative_dates);
    g.short_date_format = "YYYY-MM-DD".into();
    g.time_format = "HH:mm".into();
    g.time_zone = "Europe/Rome".into();
    g.show_relative_dates = false;
    g.enable_color_impaired_mode = true;
    g.calendar_week_column_header = "ddd DD/MM".into();
    let saved = client.put_general(&g).await.unwrap();
    assert_eq!(saved, g);
    let ui = v3(&server, reqwest::Method::GET, "/config/ui", None).await;
    assert_eq!(ui["shortDateFormat"], "YYYY-MM-DD");
    assert_eq!(ui["timeFormat"], "HH:mm");
    assert_eq!(ui["timeZone"], "Europe/Rome");
    assert_eq!(ui["showRelativeDates"], false);
    assert_eq!(ui["enableColorImpairedMode"], true);
    assert_eq!(ui["calendarWeekColumnHeader"], "ddd DD/MM");
    v3(
        &server,
        reqwest::Method::PUT,
        "/config/ui/1",
        Some(serde_json::json!({"longDateFormat": "dddd, D MMMM YYYY", "showRelativeDates": true})),
    )
    .await;
    let back = client.general().await.unwrap();
    assert_eq!(back.long_date_format, "dddd, D MMMM YYYY");
    assert!(back.show_relative_dates);
    assert_eq!(
        back.short_date_format, "YYYY-MM-DD",
        "absent fields are kept"
    );
    let v5 = reqwest::Client::new()
        .get(format!("{}/api/v5/settings/ui", server.base))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(v5["timeZone"], "Europe/Rome");
    g.time_zone = "Rome; DROP".into();
    assert_eq!(status(client.put_general(&g).await.unwrap_err()), 422);
}

#[tokio::test]
async fn host_settings_are_saved_to_the_config_file() {
    let server = start().await;
    let client = signed_in(&server).await;
    let h = client.host_settings().await.unwrap();
    assert_eq!(h.port, kitsunarr_config::DEFAULT_PORT);
    assert_eq!(h.log_level, "info");
    assert!(!h.restart_required);
    assert_eq!(h.overridden, vec!["urlBase"]);
    assert!(h.config_file.ends_with("config.toml"));
    let mut next = h.clone();
    next.port = 9123;
    next.log_level = "debug".into();
    next.backup_folder = server.dir.path().join("bk").display().to_string();
    let saved = client.put_host_settings(&next).await.unwrap();
    assert_eq!((saved.port, saved.log_level.as_str()), (9123, "debug"));
    assert!(saved.restart_required);
    assert_eq!(
        saved.running.port,
        kitsunarr_config::DEFAULT_PORT,
        "applies at restart"
    );
    let file = Config::load_file(&server.state.config.config_dir).unwrap();
    assert_eq!(file.server.port, 9123);
    assert_eq!(file.backup_dir(), server.dir.path().join("bk"));
    for (field, bad) in [
        ("bindAddress", {
            let mut b = next.clone();
            b.bind_address = "not an ip".into();
            b
        }),
        ("logLevel", {
            let mut b = next.clone();
            b.log_level = "chatty".into();
            b
        }),
        ("port", {
            let mut b = next.clone();
            b.port = 0;
            b
        }),
        ("backupFolder", {
            let mut b = next.clone();
            b.backup_folder = "relative/dir".into();
            b
        }),
    ] {
        match client.put_host_settings(&bad).await.unwrap_err() {
            ClientError::Api { status, body } => {
                assert_eq!(status, 422, "{field}");
                assert_eq!(body.fields[0].field, field);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn log_files_are_listed_shown_and_downloaded() {
    let server = start().await;
    let client = signed_in(&server).await;
    let dir = kitsunarr_api::log_files::folder(server.dir.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("kitsunarr.txt"), "2026-11-18 INFO started\n").unwrap();
    std::fs::write(dir.join("kitsunarr.0.txt"), "older\n").unwrap();
    std::fs::write(dir.join("notes.md"), "not a log").unwrap();
    let files = client.log_files().await.unwrap();
    let names: Vec<_> = files.iter().map(|f| f.filename.as_str()).collect();
    assert!(names.contains(&"kitsunarr.txt") && names.contains(&"kitsunarr.0.txt"));
    assert!(!names.contains(&"notes.md"));
    let current = files
        .iter()
        .find(|f| f.filename == "kitsunarr.txt")
        .unwrap();
    assert_eq!(current.size, 24);
    assert_eq!(
        client.log_file_text("kitsunarr.txt").await.unwrap(),
        "2026-11-18 INFO started\n"
    );
    assert_eq!(
        status(client.log_file_text("notes.md").await.unwrap_err()),
        404
    );
    assert_eq!(
        status(client.log_file_text("missing.txt").await.unwrap_err()),
        404
    );
    let resp = reqwest::Client::new()
        .get(client.log_file_download_url("kitsunarr.txt"))
        .header("X-Api-Key", &server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers()["content-disposition"],
        "attachment; filename=\"kitsunarr.txt\""
    );
}

#[tokio::test]
async fn parse_reads_titles_and_maps_library_episodes() {
    let server = start().await;
    let client = signed_in(&server).await;
    let pool = server.state.db.pool();
    insert_series(pool, 1, "Alpha", "standard", "continuing").await;
    sqlx::query("INSERT INTO episodes (series_id, season_number, episode_number, title) VALUES (1, 1, 2, 'Second')")
        .execute(pool)
        .await
        .unwrap();
    let r = client
        .parse("Alpha.S01E02.720p.HDTV.x264-GRP")
        .await
        .unwrap();
    let p = r.parsed.unwrap();
    assert_eq!(p.series_title, "Alpha");
    assert_eq!((p.season_numbers, p.episode_numbers), (vec![1], vec![2]));
    assert_eq!(p.quality, "HDTV-720p");
    assert_eq!(p.release_group.as_deref(), Some("GRP"));
    assert_eq!(r.series.unwrap().id, 1);
    assert_eq!(r.episodes.len(), 1);
    assert_eq!(r.episodes[0].title.as_deref(), Some("Second"));
    assert_eq!(status(client.parse("  ").await.unwrap_err()), 422);
}

async fn tvmaze_mock() -> String {
    use axum::extract::Path;
    use axum::routing::get;
    fn fixture(name: &str) -> serde_json::Value {
        let path = format!(
            "{}/../../tests/fixtures/metadata/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }
    async fn show(Path(_id): Path<String>) -> axum::Json<serde_json::Value> {
        axum::Json(fixture("tvmaze-show-169.json"))
    }
    async fn episodes(Path(_id): Path<String>) -> axum::Json<serde_json::Value> {
        axum::Json(fixture("tvmaze-show-169.json")["_embedded"]["episodes"].clone())
    }
    let app = axum::Router::new()
        .route("/shows/{id}", get(show))
        .route("/shows/{id}/episodes", get(episodes));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn manual_grouping_puts_new_titles_in_the_chosen_group() {
    let base = tvmaze_mock().await;
    let server = start_with(Metadata::with(
        reqwest::Client::new(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: base,
            anilist: "http://127.0.0.1:9/graphql".to_string(),
            ..kitsunarr_metadata::Endpoints::offline()
        },
    ))
    .await;
    let client = signed_in(&server).await;
    let media = tempfile::tempdir().unwrap();
    let root = media.path().display().to_string();
    client
        .create_root_folder(&RootFolderCreate {
            path: root.clone(),
            media_kind: None,
        })
        .await
        .unwrap();
    let group = client
        .create_collection(&CollectionEdit {
            title: "Heisenberg Saga".into(),
            overview: None,
        })
        .await
        .unwrap();
    let request = |manual_group_id| AddTitle {
        title: AddTitleRequest {
            provider: MetadataProvider::TvMaze,
            id: "169".into(),
            root_folder_path: root.clone(),
            quality_profile_id: 1,
            monitor: MonitorOption::None,
            series_type: None,
            season_folder: true,
            tags: vec![],
            search_for_missing: false,
            episode_ordering: None,
            path: None,
        },
        automatic_grouping: true,
        manual_group_id,
    };
    // A group that does not exist is refused before anything is added.
    assert_eq!(
        status(
            client
                .add_title_with(&request(Some(999)))
                .await
                .unwrap_err()
        ),
        404
    );
    let added = client
        .add_title_with(&request(Some(group.id)))
        .await
        .unwrap();
    let s = client.series_detail(added.id).await.unwrap();
    assert_eq!(
        s.item.path,
        format!("{root}/Heisenberg Saga/Season 01"),
        "the group folder and the member's season folder"
    );
    assert!(!s.season_folder);
    let g = s.group_membership.unwrap();
    assert_eq!((g.collection_id, g.kind.as_str()), (group.id, "manual"));
    assert_eq!(g.role.as_deref(), Some("season"));
    assert!(s.automatic_grouping);
}
