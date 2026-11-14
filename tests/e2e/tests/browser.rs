//! Browser end-to-end test: drives the real Dioxus UI in headless Chromium through WebDriver.
//!
//! Requirements: the `kitsunarr` binary built with the web UI (`dx build` + `cargo build`),
//! `chromedriver` and Chromium/Chrome. Run with
//! `cargo test -p kitsunarr-e2e -- --ignored --nocapture`.
//! Environment: `KITSUNARR_BIN` (binary path), `CHROMEDRIVER` (default `chromedriver`),
//! `CHROME_BIN` (browser binary), `KITSUNARR_E2E_SCREENSHOTS` (directory for screenshots),
//! `KITSUNARR_E2E_LOG` (server log filter, default `warn`).
//!
//! Metadata catalogues are replaced by a local mock replaying recorded TVmaze responses, so the
//! server never contacts the public catalogues. (The browser may still fetch the poster images
//! referenced by the recorded responses; nothing asserts on them.)
//!
//! The scenario performs more than six full page loads. It therefore also guards against
//! event streams of earlier pages staying open: browsers allow six HTTP/1.1 connections per
//! host, and leaked streams made later requests stall (see `kitsunarr-web/src/events.rs`).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use fantoccini::{Client, ClientBuilder, Locator};

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Prints a timestamped progress line (shown with `--nocapture`), to make slow steps visible.
fn step(what: &str) {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    eprintln!("[e2e {:>6.1}s] {what}", start.elapsed().as_secs_f64());
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

async fn wait_http(url: &str) {
    for _ in 0..100 {
        if reqwest::get(url)
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{url} did not become ready");
}

async fn screenshot(client: &Client, name: &str) {
    if let Ok(dir) = std::env::var("KITSUNARR_E2E_SCREENSHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        let png = client.screenshot().await.unwrap();
        std::fs::write(Path::new(&dir).join(format!("{name}.png")), png).unwrap();
    }
}

/// Change Chromium's real browser zoom. Resizing alone misses zoom-dependent viewport and
/// device-pixel-ratio behavior. The settings API is available on Chromium's own settings page.
async fn browser_zoom(client: &Client, percent: u16) {
    client.goto("chrome://settings/appearance").await.unwrap();
    let answer = client.execute_async(
        "const done = arguments[arguments.length - 1]; \
         chrome.settingsPrivate.setDefaultZoom(arguments[0], () => done(chrome.runtime.lastError?.message || 'ok'));",
        vec![serde_json::json!(f64::from(percent) / 100.0)],
    ).await.unwrap();
    assert_eq!(answer, "ok", "could not set browser zoom to {percent}%");
}

async fn horizontal_layout(client: &Client) -> serde_json::Value {
    client
        .execute(
            r#"
        const width = document.documentElement.clientWidth;
        const offenders = [...document.querySelectorAll('main *, .content *, [role=dialog] *')]
          .filter(e => {
            const r = e.getBoundingClientRect();
            return r.width && (r.right > width + 1 || r.left < -1)
              && !e.closest('.table-wrap, .vlist, .sidebar');
          }).slice(0, 8).map(e => ({tag: e.tagName, class: e.className,
            text: e.textContent.slice(0, 70), width: e.getBoundingClientRect().width}));
        return {scroll: document.documentElement.scrollWidth, inner: width,
          dpr: devicePixelRatio, offenders,
          mobile: matchMedia('(max-width: 900px)').matches,
          navVisible: getComputedStyle(document.querySelector('[aria-label="Open navigation"]')).display !== 'none'};
    "#,
            vec![],
        )
        .await
        .unwrap()
}

async fn modal_layout(client: &Client) -> serde_json::Value {
    client.execute(r#"
        const m = document.querySelector('[role=dialog]');
        const rect = m.getBoundingClientRect();
        const fit = e => { const r = e.getBoundingClientRect();
          return r.left >= -1 && r.right <= innerWidth + 1 && r.top >= -1 && r.bottom <= innerHeight + 1; };
        const body = m.querySelector('.modal-body');
        const style = getComputedStyle(body);
        const readableHeight = body.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
        return {fits: fit(m), bodyFits: body.scrollWidth <= body.clientWidth + 1,
          bodyReadable: readableHeight >= 2 * parseFloat(style.lineHeight), bodyHeight: readableHeight,
          footerFits: [...m.querySelectorAll('.modal-foot button')].every(fit),
          top: rect.top, bottom: rect.bottom, width: rect.width, viewport: [innerWidth, innerHeight]};
    "#, vec![]).await.unwrap()
}

async fn wait_text(client: &Client, text: &str) {
    step(&format!("waiting for {text:?}"));
    let xpath = format!("//*[contains(normalize-space(.), {})]", xpath_literal(text));
    if let Err(e) = client
        .wait()
        .at_most(Duration::from_secs(30))
        .for_element(Locator::XPath(&xpath))
        .await
    {
        // Leave evidence for debugging (locally and in CI artifacts).
        screenshot(client, "failure").await;
        let url = client
            .current_url()
            .await
            .map(|u| u.to_string())
            .unwrap_or_default();
        panic!("text {text:?} not found on {url}: {e}");
    }
}

fn xpath_literal(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{s}\"")
    } else {
        format!("'{s}'")
    }
}

async fn click_button(client: &Client, label: &str) {
    step(&format!("clicking {label:?}"));
    let xpath = format!(
        "//button[contains(normalize-space(.), {})]",
        xpath_literal(label)
    );
    let el = client
        .wait()
        .at_most(Duration::from_secs(20))
        .for_element(Locator::XPath(&xpath))
        .await
        .unwrap_or_else(|e| panic!("button {label:?} not found: {e}"));
    el.click().await.unwrap();
}

async fn fill(client: &Client, css: &str, value: &str) {
    let el = client
        .wait()
        .at_most(Duration::from_secs(20))
        .for_element(Locator::Css(css))
        .await
        .unwrap();
    el.clear().await.unwrap();
    el.send_keys(value).await.unwrap();
}

/// Serves recorded TVmaze responses: search, Breaking Bad (169) and Talking Bad (1003).
async fn tvmaze_mock(fixtures: PathBuf) -> String {
    use axum::extract::{Path as UrlPath, State};
    use axum::http::StatusCode;
    use axum::routing::get;
    type Fixtures = State<std::sync::Arc<PathBuf>>;
    fn load(dir: &Path, name: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap()
    }
    async fn search(State(dir): Fixtures) -> axum::Json<serde_json::Value> {
        axum::Json(load(&dir, "tvmaze-search.json"))
    }
    async fn show(
        State(dir): Fixtures,
        UrlPath(id): UrlPath<String>,
    ) -> Result<axum::Json<serde_json::Value>, StatusCode> {
        match id.as_str() {
            "169" => Ok(axum::Json(load(&dir, "tvmaze-show-169.json"))),
            "1003" => Ok(axum::Json(load(&dir, "tvmaze-show-1003.json"))),
            _ => Err(StatusCode::NOT_FOUND),
        }
    }
    async fn episodes(
        State(dir): Fixtures,
        UrlPath(id): UrlPath<String>,
    ) -> Result<axum::Json<serde_json::Value>, StatusCode> {
        match id.as_str() {
            "169" => Ok(axum::Json(
                load(&dir, "tvmaze-show-169.json")["_embedded"]["episodes"].clone(),
            )),
            "1003" => Ok(axum::Json(load(&dir, "tvmaze-episodes-1003.json"))),
            _ => Err(StatusCode::NOT_FOUND),
        }
    }
    // SkyHook (TheTVDB), under /skyhook: the recorded Breaking Bad (81189).
    async fn skyhook_show(
        State(dir): Fixtures,
        UrlPath((_lang, id, _season_type)): UrlPath<(String, String, String)>,
    ) -> Result<axum::Json<serde_json::Value>, StatusCode> {
        match id.as_str() {
            "81189" => Ok(axum::Json(load(&dir, "skyhook-show-81189.json"))),
            _ => Err(StatusCode::NOT_FOUND),
        }
    }
    let app = axum::Router::new()
        .route("/search/shows", get(search))
        .route("/shows/{id}", get(show))
        .route("/shows/{id}/episodes", get(episodes))
        .route(
            "/skyhook/shows/{lang}/{id}/{season_type}",
            get(skyhook_show),
        )
        .with_state(std::sync::Arc::new(fixtures));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// Serves Sonarr's recorded Newznab capabilities and feed for any search.
async fn newznab_mock(fixtures: PathBuf) -> String {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use axum::routing::get;
    async fn api(
        State(dir): State<std::sync::Arc<PathBuf>>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        let file = if q.get("t").map(String::as_str) == Some("caps") {
            "newznab_caps.xml"
        } else {
            "newznab_nzb_su.xml"
        };
        let mut body = std::fs::read_to_string(dir.join(file)).unwrap();
        // The recorded feed is a single page: later pages are empty, which ends paging.
        // (Repeating the full page would page to the 1,000-result cap, 2 s per request.)
        let offset: u32 = q.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
        if offset > 0
            && let (Some(start), Some(end)) = (body.find("<item>"), body.rfind("</item>"))
        {
            body.replace_range(start..end + "</item>".len(), "");
        }
        ([("content-type", "application/rss+xml")], body).into_response()
    }
    let app = axum::Router::new()
        .route("/api", get(api))
        .with_state(std::sync::Arc::new(fixtures));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
#[ignore = "needs chromedriver, Chromium and a server build that includes the web UI"]
async fn first_run_setup_migration_and_library() {
    let root = workspace();
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    for p in ["tv/Breaking Bad", "tv/Chernobyl", "anime/Cowboy Bebop"] {
        std::fs::create_dir_all(media.join(p)).unwrap();
    }

    let bin = std::env::var("KITSUNARR_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target/debug/kitsunarr"));
    let port = free_port();
    let tvmaze = tvmaze_mock(root.join("tests/fixtures/metadata")).await;
    let newznab = newznab_mock(root.join("tests/fixtures/indexers/sonarr")).await;
    let _server = Process(
        Command::new(&bin)
            .args([
                "--config-dir",
                tmp.path().to_str().unwrap(),
                "--data-dir",
                tmp.path().join("data").to_str().unwrap(),
            ])
            .args(["--port", &port.to_string(), "--bind", "127.0.0.1"])
            .env(
                "KITSUNARR_LOG",
                std::env::var("KITSUNARR_E2E_LOG").unwrap_or_else(|_| "warn".into()),
            )
            .env("KITSUNARR_TVMAZE_URL", &tvmaze)
            .env("KITSUNARR_TVDB_URL", format!("{tvmaze}/skyhook"))
            // Closed ports: nothing in this test may reach the real catalogues.
            .env("KITSUNARR_ANILIST_URL", "http://127.0.0.1:9/graphql")
            .env("KITSUNARR_TMDB_URL", "http://127.0.0.1:9")
            .env("KITSUNARR_JIKAN_URL", "http://127.0.0.1:9")
            .env("KITSUNARR_OMDB_URL", "http://127.0.0.1:9")
            .stdout(if std::env::var_os("KITSUNARR_E2E_LOG").is_some() {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
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
        opts["binary"] = serde_json::Value::String(chrome);
    }
    let mut caps = serde_json::Map::new();
    caps.insert("goog:chromeOptions".into(), opts);
    let client = ClientBuilder::rustls()
        .unwrap()
        .capabilities(caps)
        .connect(&format!("http://127.0.0.1:{driver_port}"))
        .await
        .expect("WebDriver session");

    let result = async {
        // First run: the app redirects to account creation.
        client.goto(&base).await.unwrap();
        wait_text(&client, "Create the administrator account").await;
        assert!(
            client
                .current_url()
                .await
                .unwrap()
                .path()
                .ends_with("/setup")
        );
        screenshot(&client, "01-setup").await;
        fill(&client, "#u", "admin").await;
        fill(&client, "#p", "correct horse battery").await;
        fill(&client, "#c", "correct horse battery").await;
        click_button(&client, "Create account").await;
        wait_text(&client, "Your library is empty").await;
        screenshot(&client, "02-dashboard-empty").await;

        // Migration wizard with the real Sonarr 4.0.20 backup.
        client.goto(&format!("{base}/import")).await.unwrap();
        wait_text(&client, "Choose a backup file").await;
        screenshot(&client, "03-import-source").await;
        let backup = root.join("tests/fixtures/sonarr-4.0.20/backup.zip");
        let input = client.find(Locator::Css("#backup-file")).await.unwrap();
        input.send_keys(backup.to_str().unwrap()).await.unwrap();
        wait_text(&client, "Path mappings").await;
        wait_text(&client, "Sonarr 4.0.20.3014").await;
        let targets = client
            .find_all(Locator::Css("input[aria-label='Kitsunarr path']"))
            .await
            .unwrap();
        assert_eq!(targets.len(), 2, "one mapping per source root folder");
        let sources = client
            .find_all(Locator::Css("input[aria-label='Source path']"))
            .await
            .unwrap();
        for (src, dst) in sources.iter().zip(&targets) {
            let source_path = src.prop("value").await.unwrap().unwrap_or_default();
            let mapped = media.join(source_path.trim_start_matches("/media/"));
            dst.clear().await.unwrap();
            dst.send_keys(mapped.to_str().unwrap()).await.unwrap();
        }
        screenshot(&client, "04-import-configure").await;
        click_button(&client, "Preview migration").await;
        wait_text(&client, "The dry run completed without blocking problems").await;
        screenshot(&client, "05-import-preview").await;
        click_button(&client, "Apply migration").await;
        wait_text(&client, "Kitsunarr will back up its own database").await;
        let confirm = client
            .find(Locator::XPath(
                "//div[@class='modal-foot']//button[normalize-space(.)='Apply']",
            ))
            .await
            .unwrap();
        confirm.click().await.unwrap();
        wait_text(&client, "all checks passed").await;
        screenshot(&client, "06-import-report").await;

        // Library shows the imported series, with pagination metadata.
        client.goto(&format!("{base}/series")).await.unwrap();
        client
            .wait()
            .at_most(Duration::from_secs(20))
            .for_element(Locator::Css(".media-card"))
            .await
            .unwrap();
        let cards = client.find_all(Locator::Css(".media-card")).await.unwrap();
        assert_eq!(
            cards.len(),
            2,
            "Breaking Bad and Chernobyl (Cowboy Bebop is anime)"
        );
        wait_text(&client, "1–2 of 2").await;
        screenshot(&client, "07-library-series").await;
        client.goto(&format!("{base}/anime")).await.unwrap();
        wait_text(&client, "Cowboy Bebop").await;

        // Series detail with seasons and episodes.
        client.goto(&format!("{base}/series")).await.unwrap();
        let card = client
            .wait()
            .for_element(Locator::XPath(
                "//a[contains(@class,'media-card')][contains(., 'Breaking Bad')]",
            ))
            .await
            .unwrap();
        card.click().await.unwrap();
        wait_text(&client, "Seasons").await;
        wait_text(&client, "Season 1").await;
        wait_text(&client, "Bluray-1080p").await;
        screenshot(&client, "08-series-detail").await;

        // Add media: the migrated (TheTVDB-owned) Breaking Bad is recognised in TVmaze results
        // through its shared TheTVDB id; a new show is added with an exact folder preview.
        client.goto(&format!("{base}/add")).await.unwrap();
        wait_text(&client, "Find something to add").await;
        // TheTVDB is the default catalogue; TMDB waits for its API key.
        wait_text(&client, "provided by TheTVDB.com").await;
        let catalogue = client
            .find(Locator::Css("select[aria-label='Catalogue']"))
            .await
            .unwrap();
        assert!(
            client
                .source()
                .await
                .unwrap()
                .contains("TMDB (TV) (API key needed)")
        );
        catalogue.select_by_value("tvMaze").await.unwrap();
        wait_text(&client, "TV data provided by TVmaze.com").await;
        fill(&client, "#add-search", "breaking bad").await;
        click_button(&client, "Search").await;
        wait_text(&client, "Talking Bad").await;
        client
            .find(Locator::XPath(
                "//article[contains(@class,'result')][.//h3[normalize-space(.)='Breaking Bad']]//a[contains(.,'In library')]",
            ))
            .await
            .expect("Breaking Bad is reported as already in the library");
        screenshot(&client, "09-add-search").await;
        client
            .find(Locator::Css("button[aria-label='Add Talking Bad']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        wait_text(&client, "Add Talking Bad (2013)").await;
        let tv_root = media.join("tv").to_str().unwrap().to_string();
        client
            .wait()
            .for_element(Locator::Css(".modal select.select"))
            .await
            .unwrap()
            .select_by_value(&tv_root)
            .await
            .unwrap();
        let preview = client
            .wait()
            .for_element(Locator::Css(".path-preview"))
            .await
            .unwrap();
        let preview_text = preview.text().await.unwrap();
        assert!(
            preview_text.starts_with(&tv_root) && preview_text.contains("Talking Bad"),
            "folder preview {preview_text:?}"
        );
        screenshot(&client, "10-add-dialog").await;
        click_button(&client, "Add to library").await;
        wait_text(&client, "Talking Bad was added to the library").await;
        wait_text(&client, "Season 1").await;
        wait_text(&client, "TVmaze · 1003").await;
        click_button(&client, "Refresh metadata").await;
        wait_text(&client, "0 new episodes, 8 updated").await;
        screenshot(&client, "11-added-series").await;
        assert!(
            std::fs::read_dir(media.join("tv"))
                .unwrap()
                .all(|e| e.unwrap().file_name() != "Talking Bad"),
            "adding a title never creates folders"
        );

        // The migrated (TheTVDB-owned) series refreshes through SkyHook.
        client.goto(&format!("{base}/series")).await.unwrap();
        client
            .wait()
            .for_element(Locator::XPath(
                "//a[contains(@class,'media-card')][contains(., 'Breaking Bad')]",
            ))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        client
            .wait()
            .for_element(Locator::XPath(
                "//button[normalize-space(.)='Refresh metadata']",
            ))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        wait_text(&client, "0 new episodes, 80 updated").await;
        screenshot(&client, "11b-refresh-thetvdb").await;
        // Disk scan and the rename preview of the migrated files.
        client
            .wait()
            .for_element(Locator::XPath("//button[normalize-space(.)='Scan disk']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        wait_text(&client, "Disk scanned").await;
        client
            .wait()
            .for_element(Locator::XPath("//button[normalize-space(.)='Rename files']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        client
            .wait()
            .for_element(Locator::XPath(
                "//div[contains(@class,'modal')][.//*[contains(.,'→') or contains(.,'already follows')]]",
            ))
            .await
            .expect("the rename preview opens");
        screenshot(&client, "11d-rename-preview").await;
        click_button(&client, "Cancel").await;
        // TheTVDB series can switch episode order (the choices need no catalogue request).
        click_button(&client, "Episode order").await;
        wait_text(&client, "Aired order (TheTVDB default)").await;
        wait_text(&client, "Absolute order").await;
        click_button(&client, "Cancel").await;
        // The catalogue dialog fetches a read-only preview and shows episode matches.
        click_button(&client, "Change catalogue").await;
        client.wait().for_element(Locator::Css("select[aria-label='Target catalogue'] option[value='tvMaze']")).await.unwrap();
        client.wait().for_element(Locator::Css("select[aria-label='Target catalogue']")).await.unwrap()
            .select_by_value("tvMaze").await.unwrap();
        fill(&client,"input[aria-label='Target catalogue ID']","169").await;
        click_button(&client,"Preview").await;
        wait_text(&client,"The catalogues share an external identity.").await;
        wait_text(&client,"Episode matches").await;
        screenshot(&client,"11i-catalogue-preview").await;
        click_button(&client,"Cancel").await;
        // Profiles carry Tanukarr's codec factors (migrated) and per-quality size limits.
        client
            .goto(&format!("{base}/settings/profiles"))
            .await
            .unwrap();
        click_button(&client, "Kitsunarr Fixture 1080p").await;
        wait_text(&client, "Codec size factors").await;
        wait_text(&client, "Size limits").await;
        click_button(&client, "Cancel").await;
        wait_text(&client, "Delay profiles").await;
        wait_text(&client, "Release profiles").await;
        wait_text(&client, "Quality presets").await;
        // Library tools: switches, storage outlook, library check, recycle bin, recent searches.
        client.goto(&format!("{base}/system/library")).await.unwrap();
        wait_text(&client, "Pause automation").await;
        wait_text(&client, "Storage outlook").await;
        click_button(&client, "Check library").await;
        wait_text(&client, "series and").await;
        wait_text(&client, "Recent searches").await;
        screenshot(&client, "11f-library-tools").await;
        client.goto(&format!("{base}/activity/manualimport")).await.unwrap();
        wait_text(&client, "Scan folder").await;
        // Manual grouping: create a group and open its editor.
        client.goto(&format!("{base}/collections")).await.unwrap();
        fill(&client, "input[aria-label='New group title']", "E2E Group").await;
        click_button(&client, "Create group").await;
        client.wait().for_element(Locator::Css("details summary")).await.unwrap().click().await.unwrap();
        wait_text(&client, "Add a title").await;
        wait_text(&client, "Delete group").await;
        let value = client.execute("return document.querySelector(\"select[aria-label='Title to add'] option[value^='s']\").value", vec![]).await.unwrap();
        client.find(Locator::Css("select[aria-label='Title to add']")).await.unwrap().select_by_value(value.as_str().unwrap()).await.unwrap();
        click_button(&client, "Add").await;
        client.wait().for_element(Locator::Css("section[aria-label='Seasons'] article.media-card")).await.unwrap();
        wait_text(&client, "Season 1").await;
        // Monitoring acts on the native season, and the refreshed card reflects it.
        client.find(Locator::Css("article.media-card label.check")).await.unwrap().click().await.unwrap();
        client.wait().for_element(Locator::Css("article.media-card .unmonitored")).await.unwrap();
        screenshot(&client, "11g-collection-editor").await;
        client.find(Locator::Css("article.media-card label.check")).await.unwrap().click().await.unwrap();
        client.wait().for_element(Locator::Css("article.media-card:first-child .poster:not(:has(.unmonitored))")).await.unwrap();
        // A local film fixture exercises real file operations through the web UI.
        // Opening the server's disposable SQLite database only prepares the fixture.
        let film_root = media.join("movies");
        let film_folder = film_root.join("E2E Film");
        std::fs::create_dir_all(&film_folder).unwrap();
        let original = film_folder.join("source.1080p.WEB-DL-GRP.mkv");
        std::fs::write(&original,b"feature").unwrap();
        std::fs::write(original.with_extension("it.srt"),b"subtitle").unwrap();
        let fixture_db = kitsunarr_db::Database::open(&tmp.path().join("data/kitsunarr.db")).await.unwrap();
        kitsunarr_db::root_folders::create(fixture_db.pool(),film_root.to_str().unwrap(),None).await.unwrap();
        let mut film_media: serde_json::Value = kitsunarr_db::settings::get_or_default(fixture_db.pool(),kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT).await.unwrap();
        film_media["enableMediaInfo"] = false.into();
        film_media["recycleBin"] = tmp.path().join("recycled").display().to_string().into();
        kitsunarr_db::settings::set(fixture_db.pool(),kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,&film_media).await.unwrap();
        kitsunarr_db::sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (900,'E2E Film','e2e film','e2efilm','e2e-film',2020,'tmdbMovie','900',(SELECT MIN(id) FROM quality_profiles),?,?,'2026-11-14T00:00:00Z')")
            .bind(film_root.to_str().unwrap()).bind(film_folder.to_str().unwrap()).execute(fixture_db.pool()).await.unwrap();
        fixture_db.close().await;
        client.goto(&format!("{base}/movies/900")).await.unwrap();
        wait_text(&client,"E2E Film").await;
        click_button(&client,"Rescan disk").await;
        wait_text(&client,"1 files added").await;
        wait_text(&client,"source.1080p.WEB-DL-GRP.mkv").await;
        click_button(&client,"Rename file").await;
        client.wait().for_element(Locator::Css(".modal .mono")).await.unwrap();
        screenshot(&client,"11j-movie-rename-preview").await;
        client.find(Locator::XPath("//div[@class='modal-foot']//button[normalize-space(.)='Rename']")).await.unwrap().click().await.unwrap();
        wait_text(&client,"1 file renamed.").await;
        assert!(!original.exists());
        let renamed: Vec<_> = std::fs::read_dir(&film_folder).unwrap().flatten().map(|e|e.path()).filter(|p|p.extension().is_some_and(|e|e=="mkv")).collect();
        assert_eq!(renamed.len(),1);
        assert!(renamed[0].with_extension("it.srt").is_file());
        click_button(&client,"Delete file").await;
        client.find(Locator::XPath("//div[@class='modal-foot']//button[normalize-space(.)='Delete']")).await.unwrap().click().await.unwrap();
        wait_text(&client,"File deleted.").await;
        assert!(!renamed[0].exists());
        assert!(tmp.path().join("recycled/E2E Film").join(renamed[0].file_name().unwrap()).is_file());
        click_button(&client,"Remove from library").await;
        let delete_option = client.find(Locator::Css(".modal input[type='checkbox']")).await.unwrap();
        assert_eq!(delete_option.prop("checked").await.unwrap().as_deref(),Some("false"));
        client.find(Locator::Css(".modal label.check")).await.unwrap().click().await.unwrap();
        screenshot(&client,"11k-movie-remove-options").await;
        click_button(&client,"Cancel").await;
        click_button(&client,"Remove from library").await;
        assert_eq!(client.find(Locator::Css(".modal input[type='checkbox']")).await.unwrap().prop("checked").await.unwrap().as_deref(),Some("false"));
        click_button(&client,"Cancel").await;
        // Metadata files (Sonarr's Settings › Metadata): migrated consumers, all off.
        client
            .goto(&format!("{base}/settings/metadata"))
            .await
            .unwrap();
        wait_text(&client, "Metadata files").await;
        wait_text(&client, "Kodi (XBMC) / Emby").await;
        wait_text(&client, "WDTV").await;
        // The calendar's iCal link carries the API key.
        client.goto(&format!("{base}/calendar")).await.unwrap();
        click_button(&client, "iCal link").await;
        wait_text(&client, "Subscribe to this address").await;
        let feed = client
            .wait()
            .at_most(Duration::from_secs(20))
            .for_element(Locator::Css("input[aria-label='iCal feed address']"))
            .await
            .unwrap()
            .prop("value")
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(feed.contains("/feed/v3/calendar/Kitsunarr.ics?pastDays=7&futureDays=28&apikey="), "{feed}");
        click_button(&client, "Close").await;
        // Auto tagging (Sonarr's Settings › Tags): a rule with a genre condition.
        client.goto(&format!("{base}/settings/tags")).await.unwrap();
        click_button(&client, "Add auto tag").await;
        fill(&client, "input[aria-label='Auto tag name']", "E2E anime").await;
        client
            .find(Locator::Css("input[aria-label='Tag anime']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        click_button(&client, "Add condition").await;
        fill(&client, "input[aria-label='Values']", "Anime, Animation").await;
        screenshot(&client, "11h-auto-tag-editor").await;
        click_button(&client, "Save").await;
        wait_text(&client, "Auto tag saved").await;
        wait_text(&client, "E2E anime").await;
        // The comma-separated values were kept as typed.
        click_button(&client, "Edit").await;
        let values = client
            .wait()
            .at_most(Duration::from_secs(20))
            .for_element(Locator::Css("input[aria-label='Values']"))
            .await
            .unwrap()
            .prop("value")
            .await
            .unwrap();
        assert_eq!(values.as_deref(), Some("Anime, Animation"));
        click_button(&client, "Cancel").await;
        // Migrated notifications are sent now (no "stored only" badge).
        client
            .goto(&format!("{base}/settings/notifications"))
            .await
            .unwrap();
        wait_text(&client, "Emby/Jellyfin, Plex and Kodi are sent").await;
        assert!(!client.source().await.unwrap().contains("stored only"));
        client.goto(&format!("{base}/series")).await.unwrap();
        client
            .goto(&format!("{base}/settings/metadata"))
            .await
            .unwrap();
        wait_text(&client, "API key needed").await;
        wait_text(&client, "TMDB API key (v3)").await;
        screenshot(&client, "11c-settings-metadata").await;
        // Import lists: list behaviour fields in the editor, clean library and exclusions.
        client
            .goto(&format!("{base}/settings/importlists"))
            .await
            .unwrap();
        wait_text(&client, "Clean library").await;
        wait_text(&client, "Exclusions").await;
        client
            .wait()
            .for_element(Locator::XPath("//button[normalize-space(.)='Add']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        click_button(&client, "AniList").await;
        wait_text(&client, "Add AniListImport").await;
        wait_text(&client, "Adding titles").await;
        client
            .wait()
            .for_element(Locator::Css("select[aria-label='Root folder']"))
            .await
            .unwrap();
        screenshot(&client, "11e-import-list-editor").await;
        click_button(&client, "Cancel").await;

        // Table view and dashboard statistics.
        client.goto(&format!("{base}/series")).await.unwrap();
        click_button(&client, "Table").await;
        client
            .wait()
            .for_element(Locator::Css("table.table tbody tr"))
            .await
            .unwrap();
        screenshot(&client, "12-library-table").await;
        client.goto(&base).await.unwrap();
        wait_text(&client, "Episodes on disk").await;
        screenshot(&client, "13-dashboard").await;

        // Theme toggle persists the attribute on <html>.
        let before = client
            .execute(
                "return document.documentElement.getAttribute('data-theme')",
                vec![],
            )
            .await
            .unwrap();
        client
            .find(Locator::Css("button[aria-label='Toggle colour theme']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        let after = client
            .execute(
                "return document.documentElement.getAttribute('data-theme')",
                vec![],
            )
            .await
            .unwrap();
        assert_ne!(before, after);

        // Settings pages render migrated providers with secrets masked.
        client
            .goto(&format!("{base}/settings/downloadclients"))
            .await
            .unwrap();
        wait_text(&client, "qBittorrent (fixture)").await;
        screenshot(&client, "14-settings-download-clients").await;
        let page = client.source().await.unwrap();
        assert!(
            !page.contains("fixture-qbit-password"),
            "secrets must never reach the browser"
        );
        wait_text(&client, "Completed and failed download handling").await;
        wait_text(&client, "Remote path mappings").await;
        // New clients start from upstream's defaults for the chosen implementation.
        client
            .wait()
            .for_element(Locator::XPath("//button[normalize-space(.)='Add']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        click_button(&client, "SABnzbd · Usenet").await;
        wait_text(&client, "Add Sabnzbd").await;
        let category = client
            .wait()
            .for_element(Locator::XPath(
                "//div[contains(@class,'field')][label[normalize-space(.)='Category']]//input",
            ))
            .await
            .unwrap();
        assert_eq!(category.prop("value").await.unwrap().as_deref(), Some("tv"));
        screenshot(&client, "14b-add-download-client").await;
        click_button(&client, "Cancel").await;

        // Indexers: point the migrated Newznab indexer at a reachable (mock) indexer, test it,
        // then search a season. The migrated Torznab indexer stays unreachable and is reported.
        client
            .goto(&format!("{base}/settings/indexers"))
            .await
            .unwrap();
        client
            .wait()
            .for_element(Locator::XPath(
                "//tr[contains(., 'NZBgeek (fixture)')]//button[normalize-space(.)='Edit']",
            ))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        let base_url = client
            .wait()
            .for_element(Locator::XPath(
                "//div[contains(@class,'field')][label[normalize-space(.)='URL']]//input",
            ))
            .await
            .unwrap();
        base_url.clear().await.unwrap();
        base_url.send_keys(&newznab).await.unwrap();
        click_button(&client, "Test").await;
        wait_text(&client, "Connected: 100 releases in the newest page").await;
        screenshot(&client, "15-indexer-test").await;
        click_button(&client, "Save").await;
        wait_text(&client, "Saved.").await;

        client.goto(&format!("{base}/series")).await.unwrap();
        // An earlier step switched the library to the (persisted) table view.
        click_button(&client, "Posters").await;
        client
            .wait()
            .for_element(Locator::XPath(
                "//a[contains(@class,'media-card')][contains(., 'Breaking Bad')]",
            ))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        client
            .wait()
            .for_element(Locator::Css("button[aria-label='Search indexers for Season 1']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        // The migrated Nyaa indexer is tagged "anime": untagged Breaking Bad does not use it.
        wait_text(&client, "Season 1: 100 releases from 1 indexer(s)").await;
        // Decisions: the recorded feed's shows are not in the library.
        wait_text(&client, "Unknown Series").await;
        let rows = client
            .find_all(Locator::Css("table.releases tbody tr"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 100);
        let page = client.source().await.unwrap();
        assert!(
            !page.contains("getnzb"),
            "indexer download links (with their keys) never reach the browser"
        );
        screenshot(&client, "16-interactive-search").await;
        // Rejected releases need a second click; this one cannot be matched to episodes.
        let grab = Locator::Css("table.releases tbody tr button[aria-label^='Grab ']");
        client.find(grab).await.unwrap().click().await.unwrap();
        wait_text(&client, "Grab anyway?").await;
        client.find(grab).await.unwrap().click().await.unwrap();
        wait_text(&client, "could not be matched to episodes").await;
        client
            .find(Locator::Css(".modal button[aria-label='Close']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();

        // A single episode, from its row.
        client
            .wait()
            .for_element(Locator::Css("button[aria-label='Search indexers for S01E01']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        wait_text(&client, "S01E01: 100 releases from 1 indexer(s)").await;
        client
            .find(Locator::Css(".modal button[aria-label='Close']"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();

        // Automatic search.
        // Centred first: notifications from earlier steps cover the bottom of the window.
        let auto = client
            .find(Locator::Css("button[aria-label='Automatic search for Season 1']"))
            .await
            .unwrap();
        client
            .execute(
                "arguments[0].scrollIntoView({block: 'center'})",
                vec![serde_json::to_value(&auto).unwrap()],
            )
            .await
            .unwrap();
        auto.click().await.unwrap();
        // Only indexers enabled for automatic search are used: the migrated NZBgeek has it off,
        // and the Nyaa indexer is for "anime"-tagged series only.
        wait_text(
            &client,
            "No indexer can search this series automatically",
        )
        .await;
        screenshot(&client, "16b-automatic-search").await;

        // The queue reads the migrated qBittorrent client (an unreachable fixture address).
        client
            .goto(&format!("{base}/activity/queue"))
            .await
            .unwrap();
        wait_text(&client, "Read live from the download clients").await;
        wait_text(&client, "qBittorrent").await;
        screenshot(&client, "17-queue").await;
        client
            .goto(&format!("{base}/activity/blocklist"))
            .await
            .unwrap();
        wait_text(&client, "The blocklist is empty").await;

        // Wanted: the migrated library's missing episodes; searching them runs in the background
        // and reports when done.
        client
            .goto(&format!("{base}/wanted/missing"))
            .await
            .unwrap();
        wait_text(&client, "Wanted").await;
        client
            .wait()
            .for_element(Locator::XPath(
                "//table//tr[.//a[normalize-space(.)='Breaking Bad']]",
            ))
            .await
            .expect("missing Breaking Bad episodes are listed");
        screenshot(&client, "18-wanted-missing").await;
        click_button(&client, "Search all").await;
        wait_text(&client, "a notification follows").await;
        wait_text(&client, "Searched ").await;
        client
            .find(Locator::LinkText("Cutoff unmet"))
            .await
            .unwrap()
            .click()
            .await
            .unwrap();
        wait_text(&client, "Cutoff unmet").await;

        // Real browser zoom, including dezoom and the narrow viewports of 300-400%.
        let mut overflowing = Vec::new();
        client.set_window_size(1366, 900).await.unwrap();
        let mut zoom_results = Vec::new();
        for zoom in [50, 67, 80, 100, 125, 150, 200, 250, 300, 400] {
            step(&format!("browser zoom {zoom}%"));
            browser_zoom(&client, zoom).await;
            for path in [
                "/", "/series", "/series/1", "/movies", "/movies/900", "/wanted/missing", "/activity/queue",
                "/activity/history", "/calendar", "/add", "/settings/indexers", "/settings/profiles",
                "/settings/notifications", "/system/tasks", "/system/library", "/activity/manualimport",
            ] {
                client.goto(&format!("{base}{path}")).await.unwrap();
                tokio::time::sleep(Duration::from_millis(700)).await;
                let wide = horizontal_layout(&client).await;
                if wide["scroll"].as_i64().unwrap() > wide["inner"].as_i64().unwrap() + 1 {
                    overflowing.push(format!("{path} at {zoom}%: {wide}"));
                }
                assert!((wide["dpr"].as_f64().unwrap() - f64::from(zoom)/100.0).abs() < 0.04,
                    "browser zoom was not applied: {wide}");
                assert_eq!(wide["navVisible"], wide["mobile"], "navigation toggle at {zoom}% on {path}: {wide}");
                zoom_results.push(serde_json::json!({"zoom":zoom,"path":path,"layout":wide}));
                if std::env::var("KITSUNARR_E2E_ZOOM_TOUR").is_ok() {
                    screenshot(&client, &format!("zoom-{zoom}{}", path.replace('/', "-"))).await;
                }
            }
            // Check controls inside the catalogue dialog at every zoom, including its footer.
            client.goto(&format!("{base}/series/1")).await.unwrap();
            click_button(&client,"Change catalogue").await;
            client.wait().for_element(Locator::Css("select[aria-label='Target catalogue'] option[value='tvMaze']")).await.unwrap();
            client.find(Locator::Css("select[aria-label='Target catalogue']")).await.unwrap().select_by_value("tvMaze").await.unwrap();
            fill(&client,"input[aria-label='Target catalogue ID']","169").await;
            click_button(&client,"Preview").await;
            wait_text(&client,"Episode matches").await;
            let modal=modal_layout(&client).await;
            if modal["fits"] != true || modal["bodyFits"] != true || modal["footerFits"] != true || modal["bodyReadable"] != true {
                overflowing.push(format!("catalogue dialog at {zoom}%: {modal}"));
            }
            zoom_results.push(serde_json::json!({"zoom":zoom,"dialog":modal}));
            screenshot(&client,&format!("zoom-{zoom}-catalogue-dialog")).await;
            click_button(&client,"Cancel").await;
        }
        if let Ok(dir) = std::env::var("KITSUNARR_E2E_SCREENSHOTS") {
            std::fs::write(Path::new(&dir).join("zoom-results.json"),serde_json::to_vec_pretty(&zoom_results).unwrap()).unwrap();
        }
        browser_zoom(&client,100).await;
        client.set_window_size(1366, 900).await.unwrap();
        // A larger default text size (20 px instead of 16) enlarges every control with it.
        for path in ["/series/1", "/wanted/missing", "/settings/indexers", "/system/tasks"] {
            client.goto(&format!("{base}{path}")).await.unwrap();
            let wide = client
                .execute(
                    "document.documentElement.style.fontSize = '20px'; \
                     return new Promise(r => setTimeout(() => r([document.documentElement.scrollWidth, window.innerWidth]), 700));",
                    vec![],
                )
                .await
                .unwrap();
            let (scroll, inner) = (wide[0].as_i64().unwrap(), wide[1].as_i64().unwrap());
            if scroll > inner {
                overflowing.push(format!("{path} with 20px text: {scroll} > {inner}"));
            }
            if std::env::var("KITSUNARR_E2E_ZOOM_TOUR").is_ok() {
                screenshot(&client, &format!("text-20px{}", path.replace('/', "-"))).await;
            }
        }
        assert!(overflowing.is_empty(), "pages scroll sideways: {overflowing:#?}");

        // The interface in Italian (Sonarr's UI languages), then back to English.
        client
            .goto(&format!("{base}/settings"))
            .await
            .unwrap();
        client
            .wait()
            .at_most(Duration::from_secs(20))
            .for_element(Locator::Css("select[aria-label='Interface language']"))
            .await
            .unwrap()
            .select_by_value("it")
            .await
            .unwrap();
        click_button(&client, "Save").await;
        wait_text(&client, "Calendario").await;
        wait_text(&client, "Lingua dell'interfaccia").await;
        screenshot(&client, "12-italian").await;
        client
            .wait()
            .at_most(Duration::from_secs(20))
            .for_element(Locator::Css("select[aria-label='Interface language']"))
            .await
            .unwrap()
            .select_by_value("en")
            .await
            .unwrap();
        click_button(&client, "Salva").await;
        wait_text(&client, "Interface language").await;

        // Sign out returns to the login page.
        click_button(&client, "Sign out").await;
        wait_text(&client, "Keep me signed in").await;
    }
    .await;
    client.close().await.ok();
    result
}
