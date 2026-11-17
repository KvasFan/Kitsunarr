//! Browser performance with a 1,000-series library (ignored by default; issue #12).
//!
//! `cargo test -p kitsunarr-e2e --test perf -- --ignored --nocapture`
//!
//! Seeds 1,000 series (750 TV, 250 anime) with 2 seasons, 25 episodes and 12 files each into
//! a real server's database, signs in with headless Chromium and measures, five times each and
//! reported as medians: a cold load of the series library (first contentful paint and the first
//! page of 60 posters), a title search typed into the header (including its 300 ms debounce),
//! clearing it, the next page, and a cold load of a series page until its episodes show.
//! Environment: as `browser.rs` (`KITSUNARR_BIN`, `CHROMEDRIVER`, `CHROME_BIN`).

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fantoccini::{Client, ClientBuilder, Locator};
use serde_json::Value;

const SERIES: i64 = 1_000;
const RUNS: usize = 5;

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_http(url: &str) {
    for _ in 0..300 {
        if reqwest::get(url)
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{url} did not answer");
}

async fn seed(path: &std::path::Path) {
    let db = kitsunarr_db::Database::open(path).await.unwrap();
    kitsunarr_db::seed::seed_defaults(&db).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    for n in 0..SERIES {
        let anime = n % 4 == 0;
        let title = format!("{} Series {n:04}", if anime { "Anime" } else { "Drama" });
        let slug = title.to_lowercase().replace(' ', "-");
        let id: i64 = kitsunarr_db::sqlx::query_scalar(
            "INSERT INTO series (media_kind, title, sort_title, clean_title, title_slug, series_type, metadata_provider, \
             metadata_id, quality_profile_id, root_folder_path, path, added_at, year, monitored, network) \
             VALUES (?, ?, ?, ?, ?, ?, 'tvdb', ?, 1, '/media/tv', ?, '2026-01-01T00:00:00Z', ?, 1, ?) RETURNING id",
        )
        .bind(if anime { "anime" } else { "series" })
        .bind(&title)
        .bind(title.to_lowercase())
        .bind(title.to_lowercase().replace(' ', ""))
        .bind(&slug)
        .bind(if anime { "anime" } else { "standard" })
        .bind((100_000 + n).to_string())
        .bind(format!("/media/tv/{title}"))
        .bind(1990 + n % 35)
        .bind(format!("Network {}", n % 40))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        for season in 1..=2 {
            kitsunarr_db::sqlx::query(
                "INSERT INTO seasons (series_id, season_number, monitored) VALUES (?, ?, 1)",
            )
            .bind(id)
            .bind(season)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        for e in 1..=25i64 {
            let season = if e <= 13 { 1 } else { 2 };
            let file: Option<i64> = if e <= 12 {
                Some(
                    kitsunarr_db::sqlx::query_scalar(
                        "INSERT INTO episode_files (series_id, season_number, relative_path, size, added_at, quality_id) \
                         VALUES (?, ?, ?, 1500000000, '2026-01-01T00:00:00Z', 7) RETURNING id",
                    )
                    .bind(id)
                    .bind(season)
                    .bind(format!("Season {season:02}/E{e:02}.mkv"))
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap(),
                )
            } else {
                None
            };
            kitsunarr_db::sqlx::query(
                "INSERT INTO episodes (series_id, season_number, episode_number, absolute_episode_number, title, \
                 air_date, air_date_utc, monitored, episode_file_id) VALUES (?, ?, ?, ?, ?, '2020-01-01', '2020-01-01T00:00:00Z', 1, ?)",
            )
            .bind(id)
            .bind(season)
            .bind(e)
            .bind(e)
            .bind(format!("Episode {e}"))
            .bind(file)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit().await.unwrap();
    db.pool().close().await;
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Runs an async script and returns its number.
async fn measure(client: &Client, script: &str) -> f64 {
    let v: Value = client.execute_async(script, vec![]).await.unwrap();
    v.as_f64()
        .unwrap_or_else(|| panic!("the measurement failed: {v}"))
}

/// Resolves once `count()` (a JS expression) reaches `n`, with the time since `t0`.
const WAIT: &str = r#"
const done = arguments[arguments.length - 1];
const t0 = performance.now();
__ACTION__
const check = () => {
  if (__CONDITION__) { done(performance.now() - t0); return; }
  if (performance.now() - t0 > 20000) { done("timed out"); return; }
  requestAnimationFrame(check);
};
requestAnimationFrame(check);
"#;

fn timed(action: &str, condition: &str) -> String {
    WAIT.replace("__ACTION__", action)
        .replace("__CONDITION__", condition)
}

/// Sets the header search box as typing does (Dioxus listens to `input` events).
const TYPE: &str = r#"
const box = document.querySelector("input[aria-label='Search the library']");
const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
set.call(box, __VALUE__);
box.dispatchEvent(new Event('input', { bubbles: true }));
"#;

#[tokio::test]
#[ignore = "needs chromedriver, Chromium and a server build that includes the web UI"]
async fn a_1000_series_library_in_the_browser() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    seed(&data.join("kitsunarr.db")).await;

    let bin = std::env::var("KITSUNARR_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target/debug/kitsunarr"));
    let port = free_port();
    let _server = Process(
        Command::new(&bin)
            .args([
                "--config-dir",
                tmp.path().to_str().unwrap(),
                "--data-dir",
                data.to_str().unwrap(),
            ])
            .args(["--port", &port.to_string(), "--bind", "127.0.0.1"])
            .env("KITSUNARR_LOG", "warn")
            .env("KITSUNARR_TVMAZE_URL", "http://127.0.0.1:9")
            .env("KITSUNARR_TVDB_URL", "http://127.0.0.1:9")
            .env("KITSUNARR_ANILIST_URL", "http://127.0.0.1:9/graphql")
            .env("KITSUNARR_TMDB_URL", "http://127.0.0.1:9")
            .env("KITSUNARR_JIKAN_URL", "http://127.0.0.1:9")
            .env("KITSUNARR_OMDB_URL", "http://127.0.0.1:9")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {}: {e}", bin.display())),
    );
    let base = format!("http://127.0.0.1:{port}");
    wait_http(&format!("{base}/api/v1/ping")).await;
    let driver_port = free_port();
    let _driver = Process(
        Command::new(std::env::var("CHROMEDRIVER").unwrap_or_else(|_| "chromedriver".into()))
            .arg(format!("--port={driver_port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("chromedriver"),
    );
    wait_http(&format!("http://127.0.0.1:{driver_port}/status")).await;
    let mut opts = serde_json::json!({
        "args": ["--headless=new", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", "--window-size=1366,900"]
    });
    if let Ok(chrome) = std::env::var("CHROME_BIN") {
        opts["binary"] = Value::String(chrome);
    }
    let mut caps = serde_json::Map::new();
    caps.insert("goog:chromeOptions".into(), opts);
    let client = ClientBuilder::rustls()
        .unwrap()
        .capabilities(caps)
        .connect(&format!("http://127.0.0.1:{driver_port}"))
        .await
        .expect("WebDriver session");

    // Sign up the administrator.
    client.goto(&base).await.unwrap();
    client.wait().for_element(Locator::Css("#u")).await.unwrap();
    for (css, value) in [
        ("#u", "admin"),
        ("#p", "correct horse battery"),
        ("#c", "correct horse battery"),
    ] {
        client
            .find(Locator::Css(css))
            .await
            .unwrap()
            .send_keys(value)
            .await
            .unwrap();
    }
    client
        .find(Locator::XPath(
            "//button[normalize-space(.)='Create account']",
        ))
        .await
        .unwrap()
        .click()
        .await
        .unwrap();
    client
        .wait()
        .at_most(Duration::from_secs(20))
        .for_element(Locator::Css(".media-card"))
        .await
        .ok();

    let cards = "document.querySelectorAll('.media-card').length";
    let (mut fcp, mut first_page, mut search, mut cleared, mut next, mut detail) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    for _ in 0..RUNS {
        // Cold load of the series library: from navigation start.
        client.goto(&format!("{base}/series")).await.unwrap();
        let loaded = measure(
            &client,
            &timed("", &format!("{cards} >= 60"))
                .replace("done(performance.now() - t0)", "done(performance.now())"),
        )
        .await;
        first_page.push(loaded);
        // The paint entry is recorded asynchronously after the paint: wait for it.
        let paint = measure(
            &client,
            r#"
const done = arguments[arguments.length - 1];
const t0 = performance.now();
const check = () => {
  const e = performance.getEntriesByName('first-contentful-paint')[0];
  if (e) { done(e.startTime); return; }
  if (performance.now() - t0 > 5000) { done("no first contentful paint"); return; }
  setTimeout(check, 50);
};
check();
"#,
        )
        .await;
        fcp.push(paint);

        // A title search, then clearing it (each includes the 300 ms debounce).
        search.push(
            measure(
                &client,
                &timed(
                    &TYPE.replace("__VALUE__", "'Series 0999'"),
                    &format!(
                        "{cards} === 1 && document.querySelector('.media-card').textContent.includes('Series 0999')"
                    ),
                ),
            )
            .await,
        );
        cleared.push(
            measure(
                &client,
                &timed(&TYPE.replace("__VALUE__", "''"), &format!("{cards} >= 60")),
            )
            .await,
        );

        // The next page of 60.
        next.push(
            measure(
                &client,
                &timed(
                    "window.__first = document.querySelector('.media-card').textContent; \
                     [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Next ›').click();",
                    &format!(
                        "{cards} >= 60 && document.querySelector('.media-card').textContent !== window.__first"
                    ),
                ),
            )
            .await,
        );

        // Cold load of a series page until its episodes show.
        client.goto(&format!("{base}/series/500")).await.unwrap();
        detail.push(
            measure(
                &client,
                &timed(
                    "",
                    "document.querySelector(\"[aria-label^='Monitor episode']\") !== null",
                )
                .replace("done(performance.now() - t0)", "done(performance.now())"),
            )
            .await,
        );
    }
    let report = [
        ("Series library: first contentful paint", median(fcp)),
        ("Series library: first 60 posters shown", median(first_page)),
        (
            "Title search to its one result (300 ms debounce included)",
            median(search),
        ),
        ("Clearing the search to 60 posters", median(cleared)),
        ("Next page of 60", median(next)),
        ("Series page: episodes shown", median(detail)),
    ];
    println!("\n{SERIES} series, median of {RUNS} runs:");
    for (what, ms) in report {
        println!("| {what} | {ms:.0} ms |");
    }
    // Only pathological regressions fail the run; the numbers go to docs/performance.md.
    assert!(report.iter().all(|(_, ms)| *ms < 10_000.0));
    client.close().await.unwrap();
}
