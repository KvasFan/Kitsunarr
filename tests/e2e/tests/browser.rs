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
    let mut stale_retries = 0;
    loop {
        let el = client
            .wait()
            .at_most(Duration::from_secs(20))
            .for_element(Locator::XPath(&xpath))
            .await
            .unwrap_or_else(|e| panic!("button {label:?} not found: {e}"));
        match el.click().await {
            Ok(()) => return,
            // Live library updates can replace a button between finding it and clicking it.
            // WebDriver rejected the stale target, so no action was dispatched to retry.
            Err(e) if e.is_stale_element_reference() && stale_retries < 3 => stale_retries += 1,
            Err(e) => panic!("button {label:?} could not be clicked: {e}"),
        }
    }
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

/// Real WebDriver pointer actions, exercising native browser drag/drop (no synthetic events).
async fn drag_quality(client: &Client, source: &str, target: &str) {
    use fantoccini::actions::{InputSource, MOUSE_BUTTON_LEFT, MouseActions, PointerAction};
    step(&format!("dragging {source} to {target}"));
    let positions = client.execute(r#"
        const s = document.querySelector(arguments[0]), t = document.querySelector(arguments[1]);
        const box = s.closest('.modal-body');
        const a = s.getBoundingClientRect(), b = t.getBoundingClientRect(), v = box.getBoundingClientRect();
        box.scrollTop += (Math.min(a.top,b.top)+Math.max(a.bottom,b.bottom))/2 - (v.top+v.bottom)/2;
        const center = e => { const r=e.getBoundingClientRect(); return [r.x+r.width/2,r.y+r.height/2]; };
        return [center(s),center(t)];
    "#,vec![serde_json::json!(source),serde_json::json!(target)]).await.unwrap();
    let point = |i: usize, axis: usize| positions[i][axis].as_f64().unwrap();
    client
        .perform_actions(
            MouseActions::new("quality-drag".into())
                .then(PointerAction::MoveTo {
                    duration: None,
                    x: point(0, 0),
                    y: point(0, 1),
                })
                .then(PointerAction::Down {
                    button: MOUSE_BUTTON_LEFT,
                })
                .pause(Duration::from_millis(100))
                .then(PointerAction::MoveBy {
                    duration: Some(Duration::from_millis(150)),
                    x: 5.0,
                    y: 3.0,
                })
                .then(PointerAction::MoveTo {
                    duration: Some(Duration::from_millis(500)),
                    x: point(1, 0),
                    y: point(1, 1),
                })
                .pause(Duration::from_millis(200))
                .then(PointerAction::MoveBy {
                    duration: Some(Duration::from_millis(100)),
                    x: 0.0,
                    y: 2.0,
                })
                .pause(Duration::from_millis(100))
                .then(PointerAction::Up {
                    button: MOUSE_BUTTON_LEFT,
                }),
        )
        .await
        .unwrap();
    client.release_actions().await.unwrap();
}

/// Leaves the saved fixture profile open, ready for the surrounding settings scenario.
async fn edit_quality_groups(client: &Client) {
    client
        .find(Locator::Css("select[aria-label='Quality cutoff']"))
        .await
        .unwrap()
        .select_by_value("7")
        .await
        .unwrap();
    click_button(client, "Edit groups").await;
    client
        .find(Locator::Css("button[aria-label='Group Bluray-1080p']"))
        .await
        .unwrap()
        .click()
        .await
        .unwrap();
    wait_text(client, "Groups need at least two qualities").await;
    assert!(
        !client
            .find(Locator::XPath(
                "//div[@class='modal-foot']//button[normalize-space(.)='Save']"
            ))
            .await
            .unwrap()
            .is_enabled()
            .await
            .unwrap()
    );
    fill(
        client,
        "[data-quality-root='g-1004'] input[aria-label='Group name']",
        "E2E 1080p",
    )
    .await;
    client
        .find(Locator::Css("select[aria-label='Group for HDTV-1080p']"))
        .await
        .unwrap()
        .select_by_value("1004")
        .await
        .unwrap();
    drag_quality(
        client,
        "button[aria-label='Drag Bluray-720p']",
        "[data-quality-root='g-1004'] .quality-group-drop",
    )
    .await;
    client
        .wait()
        .for_element(Locator::Css(
            "[data-quality-root='g-1004'] [data-quality-key='q-6']",
        ))
        .await
        .unwrap();
    client
        .find(Locator::Css(
            "button[aria-label='Prefer Bluray-1080p more']",
        ))
        .await
        .unwrap()
        .click()
        .await
        .unwrap();
    drag_quality(
        client,
        "button[aria-label='Drag E2E 1080p']",
        "[data-quality-key='g-1002']",
    )
    .await;
    let root_order = "return [...document.querySelectorAll('[data-quality-root]')].map(e=>e.dataset.qualityRoot)";
    let member_order = "return [...document.querySelectorAll(\"[data-quality-root='g-1004'] .quality-members > .quality-row\")].map(e=>e.dataset.qualityKey)";
    let roots = client.execute(root_order, vec![]).await.unwrap();
    let members = client.execute(member_order, vec![]).await.unwrap();
    assert_eq!(members, serde_json::json!(["q-6", "q-7", "q-9"]));
    let roots_list = roots.as_array().unwrap();
    assert!(
        roots_list.iter().position(|x| x == "g-1004").unwrap()
            < roots_list.iter().position(|x| x == "g-1002").unwrap(),
        "dragging a group reorders it without nesting"
    );
    assert_eq!(
        client
            .find(Locator::Css("select[aria-label='Quality cutoff']"))
            .await
            .unwrap()
            .prop("value")
            .await
            .unwrap()
            .as_deref(),
        Some("1004")
    );
    screenshot(client, "11k-quality-groups").await;
    click_button(client, "Save").await;
    wait_text(client, "Profile saved.").await;
    click_button(client, "Kitsunarr Fixture 1080p").await;
    assert_eq!(client.execute(root_order, vec![]).await.unwrap(), roots);
    assert_eq!(
        client
            .find(Locator::Css("select[aria-label='Quality cutoff']"))
            .await
            .unwrap()
            .prop("value")
            .await
            .unwrap()
            .as_deref(),
        Some("1004")
    );
    click_button(client, "Edit groups").await;
    assert_eq!(client.execute(member_order, vec![]).await.unwrap(), members);
    fill(
        client,
        "[data-quality-root='g-1004'] input[aria-label='Group name']",
        "E2E HD",
    )
    .await;
    client
        .find(Locator::Css("select[aria-label='Group for HDTV-1080p']"))
        .await
        .unwrap()
        .select_by_value("")
        .await
        .unwrap();
    client
        .find(Locator::Css("[data-quality-root='q-9']"))
        .await
        .unwrap();
    client
        .find(Locator::Css("select[aria-label='Group for WEBDL-1080p']"))
        .await
        .unwrap()
        .select_by_value("1004")
        .await
        .unwrap();
    // Moving one of two members dissolves the old singleton and preserves its other quality.
    assert!(
        client
            .find(Locator::Css("[data-quality-root='g-1002']"))
            .await
            .is_err()
    );
    client
        .find(Locator::Css("[data-quality-root='q-15']"))
        .await
        .unwrap();
    client
        .find(Locator::Css("button[aria-label='Ungroup E2E HD']"))
        .await
        .unwrap()
        .click()
        .await
        .unwrap();
    assert!(
        client
            .find(Locator::Css("[data-quality-root='g-1004']"))
            .await
            .is_err()
    );
    for id in [3, 6, 7, 9, 15] {
        client
            .find(Locator::Css(&format!("[data-quality-root='q-{id}']")))
            .await
            .unwrap();
    }
    let before_individual = client.execute(root_order, vec![]).await.unwrap();
    drag_quality(
        client,
        "button[aria-label='Drag HDTV-1080p']",
        "[data-quality-key='q-7']",
    )
    .await;
    let individual_order = client.execute(root_order, vec![]).await.unwrap();
    assert_ne!(
        before_individual, individual_order,
        "individual qualities can also be reordered by dragging"
    );
    assert_eq!(
        client
            .find(Locator::Css("select[aria-label='Quality cutoff']"))
            .await
            .unwrap()
            .prop("value")
            .await
            .unwrap()
            .as_deref(),
        Some("3")
    );
    click_button(client, "Save").await;
    wait_text(client, "Profile saved.").await;
    click_button(client, "Kitsunarr Fixture 1080p").await;
    assert_eq!(
        client.execute(root_order, vec![]).await.unwrap(),
        individual_order
    );
    assert!(
        client
            .find(Locator::Css("[data-quality-root='g-1004']"))
            .await
            .is_err()
    );
    assert_eq!(
        client
            .find(Locator::Css("select[aria-label='Quality cutoff']"))
            .await
            .unwrap()
            .prop("value")
            .await
            .unwrap()
            .as_deref(),
        Some("3")
    );
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

/// A controllable Nyaa RSS stand-in; mode 0 returns releases, 1 an empty feed, 2 HTTP 503.
async fn nyaa_mock(
    fixtures: PathBuf,
) -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicU8>,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use std::sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    };
    type Control = (Arc<AtomicU8>, Arc<AtomicUsize>, String);
    let mode = Arc::new(AtomicU8::new(0));
    let hits = Arc::new(AtomicUsize::new(0));
    let feed = std::fs::read_to_string(fixtures.join("nyaa_2021.xml")).unwrap();
    let app = axum::Router::new()
        .route(
            "/",
            axum::routing::get(
                |State((mode, hits, feed)): State<Control>,
                 Query(q): Query<std::collections::HashMap<String, String>>| async move {
                    assert_eq!(q.get("page").map(String::as_str), Some("rss"));
                    assert_eq!(q.get("term").map(String::as_str), Some("Nyaa Test Show"));
                    hits.fetch_add(1, Ordering::SeqCst);
                    if mode.load(Ordering::SeqCst) == 3 {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    match mode.load(Ordering::SeqCst) {
                        2 => axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
                        1 => (
                            [("content-type", "application/rss+xml")],
                            "<rss><channel/></rss>",
                        )
                            .into_response(),
                        _ => ([("content-type", "application/rss+xml")], feed).into_response(),
                    }
                },
            ),
        )
        .route(
            "/fail/",
            axum::routing::get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        )
        .with_state((mode.clone(), hits.clone(), feed));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mode, hits)
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
        client.find(Locator::XPath("//div[contains(@class,'hero')]//button[normalize-space(.)='History']")).await.unwrap().click().await.unwrap();
        client.wait().for_element(Locator::Css(".modal select[aria-label='Event type']")).await.unwrap();
        wait_text(&client,"Series history").await;
        screenshot(&client,"08a-series-history").await;
        client.find(Locator::Css(".modal button[aria-label='Close']")).await.unwrap().click().await.unwrap();


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
        fill(&client, "input[aria-label='Minimum score increment for upgrades']", "75").await;
        let quality_rows = "return [...document.querySelectorAll('[data-quality-root] > .quality-card > .quality-row')].map(e => e.dataset.qualityLabel)";
        let before = client.execute(quality_rows, vec![]).await.unwrap();
        client.find(Locator::Css("button[aria-label='Prefer WEB 1080p more']")).await.unwrap().click().await.unwrap();
        let reordered = client.execute(quality_rows, vec![]).await.unwrap();
        assert_ne!(before, reordered, "quality preference changed in the editor");
        client.find(Locator::XPath("//div[@role='dialog']//summary[strong[normalize-space(.)='Size limits']]")).await.unwrap().click().await.unwrap();
        fill(&client, "input[aria-label='Minimum size of 3']", "1").await;
        fill(&client, "input[aria-label='Preferred size of 3']", "10").await;
        fill(&client, "input[aria-label='Maximum size of 3']", "20").await;
        // WebDriver's clear() alone does not dispatch the input event used by Dioxus.
        // Select/delete through the keyboard to exercise the real empty-field action.
        for field in ["Minimum", "Preferred", "Maximum"] {
            let selector = format!("input[aria-label='{field} size of 3']");
            client.find(Locator::Css(&selector)).await.unwrap()
                .send_keys("\u{e009}a\u{e000}\u{e003}").await.unwrap();
        }
        click_button(&client, "Save").await;
        wait_text(&client, "Profile saved.").await;
        click_button(&client, "Kitsunarr Fixture 1080p").await;
        assert_eq!(client.find(Locator::Css("input[aria-label='Minimum score increment for upgrades']")).await.unwrap().prop("value").await.unwrap().as_deref(), Some("75"));
        assert_eq!(client.execute(quality_rows, vec![]).await.unwrap(), reordered, "quality order survives saving and reopening");
        client.find(Locator::XPath("//div[@role='dialog']//summary[strong[normalize-space(.)='Size limits']]")).await.unwrap().click().await.unwrap();
        let maximum = client.find(Locator::Css("input[aria-label='Maximum size of 3']")).await.unwrap();
        assert_eq!(maximum.prop("value").await.unwrap().as_deref(), Some(""));
        assert_eq!(maximum.attr("placeholder").await.unwrap().as_deref(), Some("unlimited"));
        assert_eq!(client.find(Locator::Css("input[aria-label='Minimum size of 3']")).await.unwrap().prop("value").await.unwrap().as_deref(), Some(""));
        screenshot(&client, "11j-quality-profile").await;
        client.find(Locator::Css("button[aria-label='Use global sizes for 3']")).await.unwrap().click().await.unwrap();
        click_button(&client, "Save").await;
        wait_text(&client, "Profile saved.").await;
        click_button(&client, "Kitsunarr Fixture 1080p").await;
        assert_eq!(client.find(Locator::Css("input[aria-label='Maximum size of 3']")).await.unwrap().attr("placeholder").await.unwrap().as_deref(), Some("global"));
        edit_quality_groups(&client).await;
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
        kitsunarr_db::quality::create_custom_format(fixture_db.pool(), &serde_json::from_value(serde_json::json!({
            "id":0,"name":"E2E WEB","includeWhenRenaming":true,"specifications":[{
                "name":"WEB","implementation":"ReleaseTitleSpecification","negate":false,"required":true,
                "fields":{"value":"(?<=\\.)WEB-DL(?=-)"}}]
        })).unwrap()).await.unwrap();
        let mut film_naming: serde_json::Value = kitsunarr_db::settings::get_or_default(fixture_db.pool(),kitsunarr_db::settings::KEY_NAMING).await.unwrap();
        film_naming["movieFileFormat"] = "{SERIES TITLE}...({Series Year}){[Custom Format:E2E WEB]}".into();
        kitsunarr_db::settings::set(fixture_db.pool(),kitsunarr_db::settings::KEY_NAMING,&film_naming).await.unwrap();
        kitsunarr_db::sqlx::query("INSERT INTO movies (id,title,sort_title,clean_title,title_slug,year,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (900,'E2E Film','e2e film','e2efilm','e2e-film',2020,'tmdbMovie','900',(SELECT MIN(id) FROM quality_profiles),?,?,'2026-11-14T00:00:00Z')")
            .bind(film_root.to_str().unwrap()).bind(film_folder.to_str().unwrap()).execute(fixture_db.pool()).await.unwrap();
        // More than one page of movie history verifies server-side paging and filter resets.
        for n in 0..51 {
            kitsunarr_db::sqlx::query("INSERT INTO history (event_type,date,source_title,movie_id) VALUES ('grabbed','2026-11-14T00:00:00Z',?,900)")
                .bind(format!("E2E release {n}")).execute(fixture_db.pool()).await.unwrap();
        }
        fixture_db.close().await;
        client.goto(&format!("{base}/movies/900")).await.unwrap();
        wait_text(&client,"E2E Film").await;
        click_button(&client,"Rescan disk").await;
        wait_text(&client,"1 files added").await;
        wait_text(&client,"source.1080p.WEB-DL-GRP.mkv").await;
        click_button(&client,"Rename file").await;
        client.wait().for_element(Locator::Css(".modal .mono")).await.unwrap();
        wait_text(&client,"E2E FILM...(2020)[E2E WEB].mkv").await;
        screenshot(&client,"11j-movie-rename-preview").await;
        client.find(Locator::XPath("//div[@class='modal-foot']//button[normalize-space(.)='Rename']")).await.unwrap().click().await.unwrap();
        wait_text(&client,"1 file renamed.").await;
        client.find(Locator::XPath("//div[contains(@class,'hero')]//button[normalize-space(.)='History']")).await.unwrap().click().await.unwrap();
        wait_text(&client,"Movie history").await;
        wait_text(&client,"1–50 of 53").await;
        client.find(Locator::XPath("//div[contains(@class,'modal')]//button[normalize-space(.)='Next ›']")).await.unwrap().click().await.unwrap();
        wait_text(&client,"51–53 of 53").await;

        let history_filter = client.wait().for_element(Locator::Css(".modal select[aria-label='Event type']")).await.unwrap();
        history_filter.select_by_value("movieFileRenamed").await.unwrap();
        client.wait().for_element(Locator::XPath("//div[contains(@class,'modal')]//tbody[count(tr)=1]/tr[td[contains(.,'movieFileRenamed')]]")).await.unwrap();
        wait_text(&client,"1–1 of 1").await;
        let rows = client.find_all(Locator::Css(".modal table tbody tr")).await.unwrap();
        assert_eq!(rows.len(),1);
        assert!(rows[0].text().await.unwrap().contains("movieFileRenamed"));
        assert!(rows[0].text().await.unwrap().contains("E2E Film"));
        screenshot(&client,"11q-movie-history").await;
        client.find(Locator::Css(".modal select[aria-label='Event type']")).await.unwrap().select_by_value("movieFileDeleted").await.unwrap();
        client.wait().for_element(Locator::XPath("//div[contains(@class,'modal')]//*[normalize-space(.)='No history']")).await.unwrap();
        client.find(Locator::Css(".modal button[aria-label='Close']")).await.unwrap().click().await.unwrap();

        // Watched state is independent of monitoring and available in both library views.
        click_button(&client,"Mark watched").await;
        client.wait().for_element(Locator::XPath("//button[normalize-space(.)='Mark unwatched']")).await.unwrap();
        screenshot(&client,"11o-movie-watched").await;
        client.goto(&format!("{base}/movies")).await.unwrap();click_button(&client,"Table").await;
        client.wait().for_element(Locator::Css("select[aria-label='Filter by watched state']")).await.unwrap().select_by_value("yes").await.unwrap();
        client.wait().for_element(Locator::Css("input[aria-label='Select E2E Film']")).await.unwrap().click().await.unwrap();
        click_button(&client,"Mark unwatched").await;wait_text(&client,"No matches").await;
        client.find(Locator::Css("select[aria-label='Filter by watched state']")).await.unwrap().select_by_value("all").await.unwrap();
        client.wait().for_element(Locator::Css("a[href='/movies/900']")).await.unwrap();
        click_button(&client,"Posters").await;
        client.wait().for_element(Locator::Css("input[aria-label='Select E2E Film']")).await.unwrap().click().await.unwrap();
        click_button(&client,"Mark watched").await;
        client.wait().for_element(Locator::Css("a.media-card .badge.success")).await.unwrap();
        screenshot(&client,"11p-movie-bulk-watched").await;
        client.find(Locator::Css("a.media-card[href='/movies/900']")).await.unwrap().click().await.unwrap();
        click_button(&client,"Mark unwatched").await;
        client.wait().for_element(Locator::XPath("//button[normalize-space(.)='Mark watched']")).await.unwrap();
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
        // Restore the recycled feature and its subtitle to the movie from Library tools.
        client.goto(&format!("{base}/system/library")).await.unwrap();
        let recycled_name=renamed[0].file_name().unwrap().to_string_lossy();
        let restore_row=format!("//tr[td[contains(., {})]]//button[contains(., 'Restore')]",xpath_literal(&recycled_name));
        client.wait().for_element(Locator::XPath(&restore_row)).await.unwrap().click().await.unwrap();
        client.wait().for_element(Locator::Css("select[aria-label='Restore media type']")).await.unwrap().select_by_value("movie").await.unwrap();
        client.wait().for_element(Locator::Css(".modal select[aria-label='Movie']")).await.unwrap().select_by_value("900").await.unwrap();
        screenshot(&client,"11l-movie-restore").await;
        client.find(Locator::Css(".modal-foot button.primary")).await.unwrap().click().await.unwrap();
        wait_text(&client,"files restored").await;
        assert_eq!(std::fs::read(&renamed[0]).unwrap(),b"feature");assert!(renamed[0].with_extension("it.srt").is_file());
        // Audit issues link to the film without changing its record or moving its files.
        let holding=renamed[0].with_extension("hold");std::fs::rename(&renamed[0],&holding).unwrap();
        click_button(&client,"Check library").await;
        client.wait().for_element(Locator::Css("a[href='/movies/900']")).await.unwrap();
        screenshot(&client,"11m-movie-audit").await;
        std::fs::rename(&holding,&renamed[0]).unwrap();
        // Manual film import selects one feature and copies its named subtitle.
        let download=tmp.path().join("manual-films");std::fs::create_dir(&download).unwrap();
        let upgrade=download.join("E2E.Film.2020.1080p.BluRay-GRP.mkv");std::fs::write(&upgrade,b"upgraded").unwrap();std::fs::write(upgrade.with_extension("ja.forced.srt"),b"sub").unwrap();
        client.goto(&format!("{base}/activity/manualimport")).await.unwrap();click_button(&client,"Movies").await;
        fill(&client,"input[aria-label='Movie source folder']",download.to_str().unwrap()).await;
        click_button(&client,"Scan folder").await;
        client.wait().for_element(Locator::Css("select[aria-label^='Movie for']")).await.unwrap().select_by_value("900").await.unwrap();
        let choice=client.wait().for_element(Locator::Css("input[type='checkbox'][aria-label^='Import ']")).await.unwrap();
        if choice.prop("checked").await.unwrap().as_deref()!=Some("true") {choice.click().await.unwrap();}
        click_button(&client,"Import 1 files").await;wait_text(&client,"1 files imported.").await;
        assert!(upgrade.is_file());assert!(upgrade.with_extension("ja.forced.srt").is_file());
        let current:Vec<_>=std::fs::read_dir(&film_folder).unwrap().flatten().map(|e|e.path()).filter(|p|p.extension().is_some_and(|e|e=="mkv")).collect();assert_eq!(current.len(),1);assert_eq!(std::fs::read(&current[0]).unwrap(),b"upgraded");assert!(current[0].with_extension("ja.forced.srt").is_file());
        assert!(tmp.path().join("recycled/E2E Film").join(renamed[0].file_name().unwrap()).is_file());
        screenshot(&client,"11n-manual-movie-import").await;
        // A second import of the same quality is refused and displayed as a warning.
        client.find(Locator::Css("input[type='checkbox'][aria-label^='Import ']")).await.unwrap().click().await.unwrap();
        click_button(&client,"Import 1 files").await;wait_text(&client,"Not imported").await;wait_text(&client,"Review the results below").await;
        assert_eq!(std::fs::read(&current[0]).unwrap(),b"upgraded");assert!(upgrade.is_file());
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

        // Guided custom formats: shared-engine previews, invalid negation, JSON round trips.
        client.goto(&format!("{base}/settings/customformats")).await.unwrap();
        click_button(&client, "+ New format").await;
        fill(&client, "input[aria-label='Custom format name']", "Anime subgroup preference").await;
        client.find(Locator::Css("select[aria-label='New condition type']")).await.unwrap()
            .select_by_value("ReleaseGroupSpecification").await.unwrap();
        click_button(&client, "Add condition").await;
        fill(&client, "input[aria-label='Condition regular expression 1']", "^SubsPlease$").await;
        fill(&client, "input[aria-label='Test release title']", "[SubsPlease] Nyaa Test Show - 01 (1080p) [ABCDEF12]").await;
        click_button(&client, "Test format").await;
        wait_text(&client, "Format matches").await;
        let preview_in_view = client.execute("const r=document.querySelector('.format-preview').getBoundingClientRect(), b=document.querySelector('.modal-body').getBoundingClientRect(); return r.top >= b.top-1 && r.bottom <= b.bottom+1;",vec![]).await.unwrap();
        assert_eq!(preview_in_view,true,"the preview result must scroll into view inside the dialog");
        screenshot(&client, "18f-format-preview").await;
        click_button(&client, "Edit JSON").await;
        let mut specs = client.execute("return JSON.parse(document.querySelector('textarea[aria-label=\"Specifications JSON\"]').value)",vec![]).await.unwrap();
        specs[0]["fields"] = serde_json::json!({"Value":"^SubsPlease$", "futureField":{"keep":true}});
        fill(&client,"textarea[aria-label='Specifications JSON']", &serde_json::to_string_pretty(&specs).unwrap()).await;
        click_button(&client, "Guided editor").await;
        assert_eq!(client.find(Locator::Css("input[aria-label='Condition regular expression 1']")).await.unwrap().prop("value").await.unwrap().as_deref(), Some("^SubsPlease$"));
        // Add an invalid required negated condition: it must never be reported as satisfied.
        client.find(Locator::Css("select[aria-label='New condition type']")).await.unwrap()
            .select_by_value("ReleaseTitleSpecification").await.unwrap();
        click_button(&client, "Add condition").await;
        fill(&client, "input[aria-label='Condition regular expression 2']", "[unclosed").await;
        for label in ["Negate condition 2", "Required condition 2"] {
            client.find(Locator::Css(&format!("input[aria-label='{label}']"))).await.unwrap().click().await.unwrap();
        }
        click_button(&client, "Test format").await;
        wait_text(&client, "Format does not match").await;
        wait_text(&client, "is not valid").await;
        client.find(Locator::Css("button[aria-label='Remove condition 2']")).await.unwrap().click().await.unwrap();
        fill(&client, "input[aria-label='Condition regular expression 1']", "^OtherGroup$").await;
        assert!(client.find_all(Locator::Css(".format-preview")).await.unwrap().is_empty(), "editing invalidates the displayed preview");
        click_button(&client, "Test format").await;
        wait_text(&client, "Format does not match").await;
        fill(&client, "input[aria-label='Condition regular expression 1']", "^SubsPlease$").await;
        click_button(&client, "Test format").await;
        wait_text(&client, "Format matches").await;
        // Invalid JSON must stay editable and must not silently replace the guided draft.
        click_button(&client, "Edit JSON").await;
        let restored = client.find(Locator::Css("textarea[aria-label='Specifications JSON']")).await.unwrap().prop("value").await.unwrap().unwrap();
        fill(&client, "textarea[aria-label='Specifications JSON']", "[").await;
        click_button(&client, "Guided editor").await;
        wait_text(&client, "Specifications must be valid JSON").await;
        fill(&client, "textarea[aria-label='Specifications JSON']", &restored).await;
        click_button(&client, "Guided editor").await;
        click_button(&client, "Save").await;
        wait_text(&client, "Custom format saved.").await;
        let saved_formats = client.execute_async("const done=arguments[arguments.length-1]; fetch('/api/v1/customformats').then(r=>r.json()).then(done);",vec![]).await.unwrap();
        let saved = saved_formats.as_array().unwrap().iter().find(|f| f["name"] == "Anime subgroup preference").unwrap();
        assert_eq!(saved["specifications"][0]["fields"]["value"], "^SubsPlease$");
        assert_eq!(saved["specifications"][0]["fields"]["futureField"]["keep"], true);
        assert!(saved["specifications"][0]["fields"].get("Value").is_none());
        // Every supported type can be added to the guided editor and removed again.
        client.find(Locator::XPath("//tr[contains(.,'Anime subgroup preference')]//button[normalize-space(.)='Edit']")).await.unwrap().click().await.unwrap();
        for kind in ["LanguageSpecification", "SourceSpecification", "ResolutionSpecification", "SizeSpecification", "IndexerFlagSpecification", "ReleaseTypeSpecification"] {
            client.find(Locator::Css("select[aria-label='New condition type']")).await.unwrap().select_by_value(kind).await.unwrap();
            click_button(&client, "Add condition").await;
        }
        assert_eq!(client.find_all(Locator::Css(".format-condition")).await.unwrap().len(), 7);
        screenshot(&client, "18g-format-editor").await;
        click_button(&client, "Cancel").await;

        // Direct native Nyaa season searches, explicit repeated requests and truthful failures.
        let (nyaa,nyaa_mode,nyaa_hits)=nyaa_mock(root.join("tests/fixtures/indexers/sonarr")).await;
        let search_db=kitsunarr_db::Database::open(&tmp.path().join("data/kitsunarr.db")).await.unwrap();
        let saved_indexers:Vec<(i64,bool)>=kitsunarr_db::sqlx::query_as("SELECT id,enable_interactive_search FROM indexers").fetch_all(search_db.pool()).await.unwrap();
        kitsunarr_db::sqlx::query("UPDATE indexers SET enable_interactive_search=0").execute(search_db.pool()).await.unwrap();
        kitsunarr_db::sqlx::query("INSERT INTO series (id,media_kind,title,sort_title,clean_title,title_slug,series_type,metadata_provider,metadata_id,quality_profile_id,root_folder_path,path,added_at) VALUES (990,'anime','Nyaa Test Show','nyaatest','nyaatest','nyaa-test','anime','myAnimeList','990',1,?,?,'2026-11-15')")
            .bind(media.display().to_string()).bind(media.join("Nyaa Test Show").display().to_string()).execute(search_db.pool()).await.unwrap();
        kitsunarr_db::sqlx::query("INSERT INTO seasons (series_id,season_number) VALUES (990,1)").execute(search_db.pool()).await.unwrap();
        for id in 1..=2 {kitsunarr_db::sqlx::query("INSERT INTO episodes (id,series_id,season_number,episode_number,absolute_episode_number) VALUES (?,990,1,?,?)").bind(9900+id).bind(id).bind(id).execute(search_db.pool()).await.unwrap();}
        kitsunarr_db::sqlx::query("INSERT INTO indexers (id,name,implementation,protocol,settings,enable_rss,enable_automatic_search,enable_interactive_search,created_at) VALUES (990,'Direct Nyaa','Nyaa','torrent',?,0,0,1,'2026-11-15')")
            .bind(serde_json::json!({"baseUrl":nyaa,"additionalParameters":"&c=1_0&f=0","animeStandardFormatSearch":false}).to_string()).execute(search_db.pool()).await.unwrap();
        client.goto(&format!("{base}/search")).await.unwrap();
        fill(&client,"input[aria-label='Series']","Nyaa Test").await;
        click_button(&client,"Nyaa Test Show").await;
        client.wait().for_element(Locator::Css("select[aria-label='Season']")).await.unwrap().select_by_value("1").await.unwrap();
        click_button(&client,"Search indexers").await;
        wait_text(&client,"Season 1: 3 releases from 1 indexer(s). 1 responding.").await;
        assert_eq!(nyaa_hits.load(std::sync::atomic::Ordering::SeqCst),1);
        click_button(&client,"Search indexers").await;
        for _ in 0..60 {if nyaa_hits.load(std::sync::atomic::Ordering::SeqCst)>=2 {break;}tokio::time::sleep(Duration::from_millis(100)).await;}
        assert_eq!(nyaa_hits.load(std::sync::atomic::Ordering::SeqCst),2,"same-target Search button makes a new request");
        wait_text(&client,"Season 1: 3 releases from 1 indexer(s). 1 responding.").await;
        screenshot(&client,"18a-direct-nyaa").await;
        let filter_hits=nyaa_hits.load(std::sync::atomic::Ordering::SeqCst);
        fill(&client,"input[aria-label='Filter releases']","no-such-subgroup").await;
        wait_text(&client,"No releases match these filters").await;
        assert_eq!(client.find_all(Locator::Css("table.releases tbody tr")).await.unwrap().len(),0);
        assert_eq!(nyaa_hits.load(std::sync::atomic::Ordering::SeqCst),filter_hits,"local filtering must not query Nyaa");
        click_button(&client,"Clear filters").await;
        wait_text(&client,"Showing 3 of 3 releases").await;
        client.find(Locator::Css("select[aria-label='Filter release decisions']")).await.unwrap().select_by_value("unmatched").await.unwrap();
        let unmatched_rows=client.find_all(Locator::Css("table.releases tbody tr")).await.unwrap().len();
        assert!(unmatched_rows>0,"the fixture includes unmatched anime names");
        screenshot(&client,"18h-release-filters").await;
        click_button(&client,"Clear filters").await;
        assert_eq!(client.find_all(Locator::Css("table.releases tbody tr")).await.unwrap().len(),3);

        client.find(Locator::Css(".search-diagnostics summary")).await.unwrap().click().await.unwrap();
        wait_text(&client,"Titles considered: Nyaa Test Show").await;
        wait_text(&client,"Tier 1: term=Nyaa Test Show").await;
        wait_text(&client,"Nyaa RSS returns only the first page").await;
        screenshot(&client,"18d-search-diagnostics").await;
        client.find(Locator::Css("input[aria-label='Search Direct Nyaa']")).await.unwrap().click().await.unwrap();
        let before=nyaa_hits.load(std::sync::atomic::Ordering::SeqCst);
        click_button(&client,"Apply sources").await;
        wait_text(&client,"No indexers to search").await;
        assert_eq!(nyaa_hits.load(std::sync::atomic::Ordering::SeqCst),before,"unselected source receives no request");
        click_button(&client,"All sources").await;
        wait_text(&client,"Season 1: 3 releases from 1 indexer(s). 1 responding.").await;
        assert_eq!(nyaa_hits.load(std::sync::atomic::Ordering::SeqCst),before+1);

        nyaa_mode.store(3,std::sync::atomic::Ordering::SeqCst);
        click_button(&client,"Search again").await;
        wait_text(&client,"Searching indexers…").await;
        client.find(Locator::Css("input[aria-label='Search Direct Nyaa']")).await.unwrap().click().await.unwrap();
        click_button(&client,"Apply sources").await;
        wait_text(&client,"No indexers to search").await;
        tokio::time::sleep(Duration::from_millis(2200)).await;
        assert!(client.source().await.unwrap().contains("No indexers to search"),"superseded results must not replace the current search");
        screenshot(&client,"18e-superseded-search").await;
        nyaa_mode.store(0,std::sync::atomic::Ordering::SeqCst);
        click_button(&client,"All sources").await;
        wait_text(&client,"Season 1: 3 releases from 1 indexer(s). 1 responding.").await;
        nyaa_mode.store(2,std::sync::atomic::Ordering::SeqCst);
        click_button(&client,"Search again").await;
        wait_text(&client,"Search sources unavailable").await;
        screenshot(&client,"18b-nyaa-unavailable").await;
        assert!(!client.source().await.unwrap().contains("The indexers returned no releases for this search."));
        nyaa_mode.store(1,std::sync::atomic::Ordering::SeqCst);
        kitsunarr_db::sqlx::query("DELETE FROM indexer_status WHERE indexer_id=990").execute(search_db.pool()).await.unwrap();
        click_button(&client,"Search again").await;
        wait_text(&client,"Nothing found").await;
        wait_text(&client,"1 responding.").await;
        kitsunarr_db::sqlx::query("INSERT INTO indexers (id,name,implementation,protocol,settings,enable_rss,enable_automatic_search,enable_interactive_search,created_at) VALUES (991,'Unavailable Nyaa','Nyaa','torrent',?,0,0,1,'2026-11-15')")
            .bind(serde_json::json!({"baseUrl":format!("{nyaa}/fail"),"animeStandardFormatSearch":false}).to_string()).execute(search_db.pool()).await.unwrap();
        click_button(&client,"Search again").await;
        wait_text(&client,"Search incomplete").await;
        screenshot(&client,"18c-nyaa-partial").await;
        client.goto(&base).await.unwrap();
        kitsunarr_db::sqlx::query("DELETE FROM indexers WHERE id IN (990,991)").execute(search_db.pool()).await.unwrap();
        kitsunarr_db::sqlx::query("DELETE FROM series WHERE id=990").execute(search_db.pool()).await.unwrap();
        for (id,enabled) in saved_indexers {kitsunarr_db::sqlx::query("UPDATE indexers SET enable_interactive_search=? WHERE id=?").bind(enabled).bind(id).execute(search_db.pool()).await.unwrap();}
        search_db.close().await;

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
                "/settings/customformats", "/settings/notifications", "/system/tasks", "/system/library", "/activity/manualimport",
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
            client.goto(&format!("{base}/settings/customformats")).await.unwrap();
            click_button(&client,"+ New format").await;
            click_button(&client,"Add condition").await;
            let format_modal=modal_layout(&client).await;
            if format_modal["fits"] != true || format_modal["bodyFits"] != true || format_modal["footerFits"] != true || format_modal["bodyReadable"] != true {
                overflowing.push(format!("custom-format dialog at {zoom}%: {format_modal}"));
            }
            zoom_results.push(serde_json::json!({"zoom":zoom,"formatDialog":format_modal}));
            screenshot(&client,&format!("zoom-{zoom}-format-dialog")).await;
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
