//! Media information on real video files made with ffmpeg (skipped when ffmpeg or ffprobe is
//! not installed): disk scans record it, the quality takes the measured resolution, the
//! `{MediaInfo …}` naming tokens use it, and short files are samples.

use std::path::Path;
use std::process::Command;

use kitsunarr_db::Database;
use kitsunarr_db::settings as kv;
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};

fn ffmpeg_available() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|b| {
        Command::new(b)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// A 720p H.264 video with Japanese 5.1 AAC and English stereo AAC, `seconds` long.
fn make_video(path: &Path, seconds: u32) {
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("color=c=black:size=1280x720:rate=1")
        .args(["-f", "lavfi", "-i", "anullsrc=r=8000:cl=5.1"])
        .args(["-f", "lavfi", "-i", "anullsrc=r=8000:cl=stereo"])
        .args(["-t", &seconds.to_string()])
        .args(["-map", "0:v", "-map", "1:a", "-map", "2:a"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(["-c:a", "aac", "-b:a", "16k"])
        .args([
            "-metadata:s:a:0",
            "language=jpn",
            "-metadata:s:a:1",
            "language=eng",
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

async fn library(dir: &Path, runtime: i32) -> Database {
    let root = dir.join("tv");
    std::fs::create_dir_all(root.join("Show/Season 01")).unwrap();
    let db = Database::open(&dir.join("k.db")).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let pool = db.pool();
    sqlx::query(
        "INSERT INTO series (id, media_kind, title, sort_title, clean_title, title_slug, metadata_provider, metadata_id, \
         quality_profile_id, root_folder_path, path, added_at, runtime) VALUES (1, 'series', 'Show', 'show', 'show', 'show', \
         'tvMaze', '1', 1, ?, ?, '2026-11-01T00:00:00Z', ?)",
    )
    .bind(root.display().to_string())
    .bind(root.join("Show").display().to_string())
    .bind(runtime)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO seasons (series_id, season_number, monitored) VALUES (1, 1, 1)")
        .execute(pool)
        .await
        .unwrap();
    for n in 1..=2 {
        sqlx::query("INSERT INTO episodes (series_id, season_number, episode_number, title) VALUES (1, 1, ?, 'x')")
            .bind(n)
            .execute(pool)
            .await
            .unwrap();
    }
    db
}

#[tokio::test]
async fn disk_scans_record_media_info_used_by_quality_and_naming() {
    if !ffmpeg_available() {
        eprintln!("ffmpeg or ffprobe is not installed: skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let db = library(dir.path(), 2).await;
    let pool = db.pool();
    let show = dir.path().join("tv/Show/Season 01");
    // No resolution or source in the name: only the file can tell.
    make_video(&show.join("Show - S01E01.mkv"), 20);

    let scan = kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    assert_eq!(scan.added, 1, "{scan:?}");
    let (quality, json): (i64, Option<String>) =
        sqlx::query_as("SELECT quality_id, media_info FROM episode_files")
            .fetch_one(pool)
            .await
            .unwrap();
    let info: kitsunarr_library::media_info::MediaInfo =
        serde_json::from_str(&json.expect("media info recorded")).unwrap();
    assert_eq!((info.width, info.height), (1280, 720));
    assert_eq!(info.audio_streams.len(), 2);
    assert!((info.run_time - 20.0).abs() < 1.0, "{}", info.run_time);
    let name = kitsunarr_domain::Quality::by_id(quality as i32)
        .unwrap()
        .name;
    assert!(name.contains("720p"), "{name}");

    // `{MediaInfo …}` tokens in the episode format.
    let mut naming: NamingSettings = kv::get_or_default(pool, kv::KEY_NAMING).await.unwrap();
    naming.standard_episode_format = "{Series Title} - S{season:00}E{episode:00} [{MediaInfo VideoCodec} {MediaInfo AudioCodec} {MediaInfo AudioChannels}]{MediaInfo AudioLanguages}".into();
    kv::set(pool, kv::KEY_NAMING, &naming).await.unwrap();
    let previews = kitsunarr_library::files::rename_previews(&db, 1, None)
        .await
        .unwrap();
    assert_eq!(previews.len(), 1);
    assert!(
        previews[0]
            .new_path
            .ends_with("Show - S01E01 [h264 AAC 5.1][JA+EN].mkv"),
        "{}",
        previews[0].new_path
    );

    // "Analyze video files" off: a new file is recorded without media information.
    let mut media: MediaManagementSettings = kv::get_or_default(pool, kv::KEY_MEDIA_MANAGEMENT)
        .await
        .unwrap();
    media.enable_media_info = false;
    kv::set(pool, kv::KEY_MEDIA_MANAGEMENT, &media)
        .await
        .unwrap();
    make_video(&show.join("Show - S01E02.mkv"), 20);
    kitsunarr_library::files::scan_series(&db, 1).await.unwrap();
    let without: Option<String> =
        sqlx::query_scalar("SELECT media_info FROM episode_files WHERE relative_path LIKE '%E02%'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(without, None);
}

/// `DetectSample`: measured runtimes decide, against the episodes' runtime.
#[tokio::test]
async fn short_files_are_samples_by_runtime() {
    if !ffmpeg_available() {
        eprintln!("ffmpeg or ffprobe is not installed: skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Show.S01E01.mkv");
    make_video(&path, 20);
    let info = kitsunarr_library::media_info::reader::read(&path)
        .await
        .unwrap();
    // A 2-minute episode needs 15 seconds; a 24-minute one 5 minutes.
    assert!(!info.is_sample(2));
    assert!(info.is_sample(24));
    assert!(info.is_sample(0), "45 minutes when unknown");
}
