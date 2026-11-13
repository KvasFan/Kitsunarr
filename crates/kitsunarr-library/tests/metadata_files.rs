//! Sonarr's metadata consumers on a real folder: a disk scan writes Kodi, WDTV and Plex files
//! (artwork from a stand-in image server), records them, keeps Kodi's watched flag, adopts
//! files already there, does not fetch artwork twice, follows renames and goes with a deleted
//! episode file.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kitsunarr_db::Database;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::{
    MediaManagementSettings, MetadataConsumersSettings, PlexMetadataSettings,
};

async fn image_server() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().fallback(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            "IMG"
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), hits)
}

async fn library(dir: &Path, images: &str) -> Database {
    let root = dir.join("tv");
    std::fs::create_dir_all(root.join("Show/Season 01")).unwrap();
    let db = Database::open(&dir.join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let pool = db.pool();
    let series_images = serde_json::json!([
        {"coverType": "poster", "remoteUrl": format!("{images}/poster.jpg")},
        {"coverType": "fanart", "remoteUrl": format!("{images}/fanart.jpg")}
    ]);
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, year, status, overview, network, \
         genres, images, metadata_provider, metadata_id, quality_profile_id, root_folder_path, path, added_at) \
         VALUES (1, 'series', 'Show', 'show', 'show', 'show', 2008, 'ended', 'An overview.', 'AMC', '[\"Drama\"]', ?, \
         'tvdb', '81189', 1, ?, ?, '2026-11-01T00:00:00Z')",
    )
    .bind(series_images.to_string())
    .bind(root.display().to_string())
    .bind(root.join("Show").display().to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series_external_ids (series_id, kind, external_id) VALUES (1, 'tvdb', '81189'), (1, 'imdb', 'tt0903747')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO seasons (series_id, season_number, monitored, images) VALUES (1, 1, 1, ?)",
    )
    .bind(
        serde_json::json!([{"coverType": "poster", "remoteUrl": format!("{images}/s1.jpg")}])
            .to_string(),
    )
    .execute(pool)
    .await
    .unwrap();
    for n in 1..=2 {
        sqlx::query(
            "INSERT INTO episodes (series_id, season_number, episode_number, title, air_date, provider_episode_id, screenshot) \
             VALUES (1, 1, ?, ?, '2008-01-20', ?, ?)",
        )
        .bind(n)
        .bind(format!("Episode {n}"))
        .bind(format!("{}", 349_230 + n))
        .bind(format!("{images}/e{n}.jpg"))
        .execute(pool)
        .await
        .unwrap();
    }
    // No ffprobe needed.
    let mut media: MediaManagementSettings = kv::get_or_default(pool, kv::KEY_MEDIA_MANAGEMENT)
        .await
        .unwrap();
    media.enable_media_info = false;
    kv::set(pool, kv::KEY_MEDIA_MANAGEMENT, &media)
        .await
        .unwrap();
    let mut consumers = MetadataConsumersSettings::default();
    consumers.kodi.enable = true;
    consumers.wdtv.enable = true;
    consumers.plex = PlexMetadataSettings {
        enable: true,
        series_plex_match_file: true,
        episode_mappings: true,
    };
    kv::set(pool, kv::KEY_METADATA_CONSUMERS, &consumers)
        .await
        .unwrap();
    db
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[tokio::test]
async fn consumers_write_track_rename_and_remove_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let (images, hits) = image_server().await;
    let db = library(dir.path(), &images).await;
    let show = dir.path().join("tv/Show");
    let season = show.join("Season 01");
    std::fs::write(season.join("Show - S01E01 - Episode 1.mkv"), b"video").unwrap();
    std::fs::write(season.join("Show - S01E02 - Episode 2.mkv"), b"video").unwrap();
    // Kodi's NFO of the second episode was written before (by Sonarr) and marked watched.
    std::fs::write(
        season.join("Show - S01E02 - Episode 2.nfo"),
        "<episodedetails><watched>true</watched></episodedetails>",
    )
    .unwrap();

    let scan = kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert_eq!(scan.added, 2);

    let tvshow = read(&show.join("tvshow.nfo"));
    assert!(tvshow.contains("<title>Show</title>"), "{tvshow}");
    assert!(tvshow.contains("<uniqueid type=\"tvdb\" default=\"true\">81189</uniqueid>"));
    assert!(tvshow.contains("<status>Ended</status>"));
    assert_eq!(read(&show.join("poster.jpg")), "IMG");
    assert_eq!(read(&show.join("fanart.jpg")), "IMG");
    assert_eq!(read(&show.join("season01-poster.jpg")), "IMG");
    let e1 = read(&season.join("Show - S01E01 - Episode 1.nfo"));
    assert!(e1.contains("<title>Episode 1</title>"), "{e1}");
    assert!(e1.contains("<uniqueid type=\"tvdb\" default=\"true\">349231</uniqueid>"));
    assert!(e1.contains("<watched>false</watched>"));
    assert!(season.join("Show - S01E01 - Episode 1-thumb.jpg").is_file());
    // The existing NFO was adopted and rewritten, keeping Kodi's watched flag.
    let e2 = read(&season.join("Show - S01E02 - Episode 2.nfo"));
    assert!(e2.contains("<title>Episode 2</title>") && e2.contains("<watched>true</watched>"));
    // WDTV: folder images and episode XML/thumbnail.
    assert!(show.join("folder.jpg").is_file());
    assert!(season.join("folder.jpg").is_file());
    assert!(
        read(&season.join("Show - S01E01 - Episode 1.xml"))
            .contains("<series_name>Show</series_name>")
    );
    assert!(season.join("Show - S01E01 - Episode 1.metathumb").is_file());
    // Plex's match file maps the files.
    let plex = read(&show.join(".plexmatch"));
    assert!(plex.contains("TvdbId: 81189\n"), "{plex}");
    assert!(plex.contains("Episode: S01E01: Season 01/Show - S01E01 - Episode 1.mkv\n"));

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metadata_files")
        .fetch_one(db.pool())
        .await
        .unwrap();
    // Kodi (8): tvshow.nfo, poster, fanart, season poster, 2 × (nfo, thumb); WDTV (6): folder.jpg,
    // season folder.jpg, 2 × (xml, metathumb); Plex (1): .plexmatch.
    let listed: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT consumer, type, relative_path FROM metadata_files ORDER BY consumer, relative_path",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(rows, 15, "{listed:#?}");

    // A second scan fetches nothing again and rewrites nothing unchanged.
    let fetched = hits.load(Ordering::SeqCst);
    let before = std::fs::metadata(show.join("tvshow.nfo"))
        .unwrap()
        .modified()
        .unwrap();
    kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), fetched);
    assert_eq!(
        std::fs::metadata(show.join("tvshow.nfo"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );

    // A rename moves the episode's metadata with it.
    let file_id: i64 =
        sqlx::query_scalar("SELECT id FROM episode_files WHERE relative_path LIKE '%S01E01%'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    std::fs::rename(
        season.join("Show - S01E01 - Episode 1.mkv"),
        season.join("Show.S01E01.mkv"),
    )
    .unwrap();
    {
        let mut conn = db.pool().acquire().await.unwrap();
        kitsunarr_db::library::series::set_file_path(
            &mut conn,
            file_id,
            "Season 01/Show.S01E01.mkv",
        )
        .await
        .unwrap();
    }
    kitsunarr_library::metadata_files::move_after_rename(&db, 1, file_id).await;
    assert!(season.join("Show.S01E01.nfo").is_file());
    assert!(season.join("Show.S01E01-thumb.jpg").is_file());
    assert!(season.join("Show.S01E01.xml").is_file());
    assert!(season.join("Show.S01E01.metathumb").is_file());
    assert!(!season.join("Show - S01E01 - Episode 1.nfo").exists());

    // Deleting the file deletes its metadata (no recycle bin set).
    kitsunarr_library::files::delete_file(&db, 1, file_id)
        .await
        .unwrap();
    for name in [
        "Show.S01E01.nfo",
        "Show.S01E01-thumb.jpg",
        "Show.S01E01.xml",
        "Show.S01E01.metathumb",
    ] {
        assert!(!season.join(name).exists(), "{name} is left");
    }
    let left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM metadata_files WHERE episode_file_id = ?")
            .bind(file_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(left, 0);
    assert!(show.join("tvshow.nfo").is_file(), "series files stay");
}

/// Films get Kodi's `movie.nfo`, poster and fanart in their folder; artwork is fetched once.
#[tokio::test]
async fn kodi_writes_a_films_nfo_and_artwork() {
    let dir = tempfile::tempdir().unwrap();
    let (images, hits) = image_server().await;
    let root = dir.path().join("movies");
    let folder = root.join("Suzume (2022)");
    std::fs::create_dir_all(&folder).unwrap();
    let db = Database::open(&dir.path().join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let pool = db.pool();
    let movie_images = serde_json::json!([
        {"coverType": "poster", "remoteUrl": format!("{images}/p.jpg")},
        {"coverType": "fanart", "remoteUrl": format!("{images}/f.png")}
    ]);
    sqlx::query(
        "INSERT INTO movies (id, title, sort_title, clean_title, title_slug, year, overview, studio, runtime, genres, images, \
         in_cinemas, metadata_provider, metadata_id, quality_profile_id, root_folder_path, path, added_at) \
         VALUES (1, 'Suzume', 'suzume', 'suzume', 'suzume', 2022, 'Doors & disasters.', 'CoMix Wave', 122, '[\"Anime\"]', ?, \
         '2022-11-11', 'tmdbMovie', '916224', 1, ?, ?, '2026-11-01T00:00:00Z')",
    )
    .bind(movie_images.to_string())
    .bind(root.display().to_string())
    .bind(folder.display().to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO movie_external_ids (movie_id, kind, external_id) VALUES (1, 'tmdb', '916224'), (1, 'imdb', 'tt16428256')")
        .execute(pool)
        .await
        .unwrap();
    let mut consumers = MetadataConsumersSettings::default();
    consumers.kodi.enable = true;
    consumers.kodi.series_metadata = true;
    consumers.kodi.series_images = true;
    kv::set(pool, kv::KEY_METADATA_CONSUMERS, &consumers)
        .await
        .unwrap();

    kitsunarr_library::metadata_files::after_movie_change(&db, 1).await;
    let nfo = std::fs::read_to_string(folder.join("movie.nfo")).unwrap();
    assert!(
        nfo.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n<movie>"),
        "{nfo}"
    );
    for part in [
        "<title>Suzume</title>",
        "<plot>Doors &amp; disasters.</plot>",
        "<runtime>122</runtime>",
        "<uniqueid type=\"tmdb\" default=\"true\">916224</uniqueid>",
        "<uniqueid type=\"imdb\">tt16428256</uniqueid>",
        "<premiered>2022-11-11</premiered>",
        "<year>2022</year>",
        "<studio>CoMix Wave</studio>",
    ] {
        assert!(nfo.contains(part), "{part} in {nfo}");
    }
    assert_eq!(std::fs::read(folder.join("poster.jpg")).unwrap(), b"IMG");
    assert!(folder.join("fanart.png").is_file());
    let recorded: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM movie_metadata_files WHERE movie_id = 1")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(recorded, 3);
    // Again: nothing is downloaded twice.
    kitsunarr_library::metadata_files::after_movie_change(&db, 1).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
