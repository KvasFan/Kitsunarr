//! Browser end-to-end test: drives the real Dioxus UI in headless Chromium through WebDriver.
//!
//! Requirements: the `kitsunarr` binary built with the web UI (`dx build` + `cargo build`),
//! `chromedriver` and Chromium/Chrome. Run with
//! `cargo test -p kitsunarr-e2e -- --ignored --nocapture`.
//! Environment: `KITSUNARR_BIN` (binary path), `CHROMEDRIVER` (default `chromedriver`),
//! `CHROME_BIN` (browser binary), `KITSUNARR_E2E_SCREENSHOTS` (directory for screenshots).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fantoccini::{Client, ClientBuilder, Locator};

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

async fn wait_text(client: &Client, text: &str) {
    let xpath = format!("//*[contains(normalize-space(.), {})]", xpath_literal(text));
    client
        .wait()
        .at_most(Duration::from_secs(30))
        .for_element(Locator::XPath(&xpath))
        .await
        .unwrap_or_else(|e| panic!("text {text:?} not found: {e}"));
}

fn xpath_literal(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{s}\"")
    } else {
        format!("'{s}'")
    }
}

async fn click_button(client: &Client, label: &str) {
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
    let _server = Process(
        Command::new(&bin)
            .args([
                "--config-dir",
                tmp.path().to_str().unwrap(),
                "--data-dir",
                tmp.path().join("data").to_str().unwrap(),
            ])
            .args(["--port", &port.to_string(), "--bind", "127.0.0.1"])
            .env("KITSUNARR_LOG", "warn")
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

        // Table view and dashboard statistics.
        client.goto(&format!("{base}/series")).await.unwrap();
        click_button(&client, "Table").await;
        client
            .wait()
            .for_element(Locator::Css("table.table tbody tr"))
            .await
            .unwrap();
        screenshot(&client, "09-library-table").await;
        client.goto(&base).await.unwrap();
        wait_text(&client, "Episodes on disk").await;
        screenshot(&client, "10-dashboard").await;

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
        screenshot(&client, "11-settings-download-clients").await;
        let page = client.source().await.unwrap();
        assert!(
            !page.contains("fixture-qbit-password"),
            "secrets must never reach the browser"
        );

        // Sign out returns to the login page.
        click_button(&client, "Sign out").await;
        wait_text(&client, "Keep me signed in").await;
    }
    .await;
    client.close().await.ok();
    result
}
