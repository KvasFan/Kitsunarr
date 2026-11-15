//! Opt-in checks against owned, loopback-only download clients. See the verification guide.
//! The source is generated test data, served by tests/fixtures/generate/download_clients.py.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kitsunarr_downloaders::{
    AddRequest, ClientSettings, Clients, DownloadItem, ItemStatus, Source,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Client {
    name: String,
    implementation: String,
    settings: Map<String, Value>,
    downloads_path: PathBuf,
}
#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    api: Option<Api>,
    sandbox: PathBuf,
    clients: Vec<Client>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Api {
    base_url: String,
    api_key: String,
}

static LIVE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Proof {
    name: String,
    bytes: u64,
    sha256: String,
    info_hash: String,
}

fn load() -> (Config, Proof, Vec<u8>, Vec<u8>) {
    let file = std::env::var("KITSUNARR_LIVE_CLIENT_CONFIG")
        .expect("set the private test-client configuration path");
    let config: Config = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let root = config.sandbox.canonicalize().unwrap();
    assert!(
        root.ends_with(".dev/download-clients"),
        "use the dedicated ignored sandbox"
    );
    assert_eq!(config.clients.len(), 5);
    for c in &config.clients {
        assert_eq!(c.settings["host"], "127.0.0.1");
        assert_eq!(c.settings["tvCategory"], "kitsunarr-client-test");
        assert!(c.downloads_path.canonicalize().unwrap().starts_with(&root));
    }
    let dir = root.join("proof");
    let proof: Proof =
        serde_json::from_slice(&std::fs::read(dir.join("proof.json")).unwrap()).unwrap();
    assert_eq!(proof.name, "kitsunarr-client-proof.bin");
    let torrent = std::fs::read(dir.join("proof.torrent")).unwrap();
    let nzb = std::fs::read(dir.join("proof.nzb")).unwrap();
    assert!(
        kitsunarr_downloaders::torrent_info_hash(&torrent)
            .unwrap()
            .eq_ignore_ascii_case(&proof.info_hash)
    );
    (config, proof, torrent, nzb)
}

fn settings(c: &Client, paused: bool) -> ClientSettings {
    let mut map = c.settings.clone();
    map.insert("initialState".into(), json!(if paused { 2 } else { 0 }));
    map.insert("addPaused".into(), json!(paused));
    if c.implementation == "Sabnzbd" {
        map.insert(
            "recentTvPriority".into(),
            json!(if paused { -2 } else { 0 }),
        );
    }
    ClientSettings::from_provider(&c.implementation, &map).unwrap()
}

async fn add(
    clients: &Clients,
    s: &ClientSettings,
    proof: &Proof,
    torrent: &[u8],
    nzb: &[u8],
) -> Result<String, String> {
    if s.is_usenet() {
        clients
            .add_nzb(s, "kitsunarr-client-proof.nzb", nzb, true)
            .await
    } else {
        clients
            .add(
                s,
                &AddRequest {
                    source: Source::File {
                        name: "kitsunarr-client-proof.torrent",
                        content: torrent,
                    },
                    title: "kitsunarr-client-proof",
                    hash: &proof.info_hash,
                    recent: true,
                    tags: &[],
                    seed: None,
                    release: None,
                },
            )
            .await
    }
    .map_err(|e| e.to_string())
}

async fn item(
    clients: &Clients,
    settings: &ClientSettings,
    id: &str,
    complete: bool,
) -> Result<DownloadItem, String> {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed() < Duration::from_secs(if complete { 90 } else { 15 }) {
        let items = clients.items(settings).await.map_err(|e| e.to_string())?;
        if let Some(item) = items
            .into_iter()
            .find(|i| i.download_id.eq_ignore_ascii_case(id))
        {
            last = format!("{:?} {:?}", item.status, item.message);
            if !complete || item.status == ItemStatus::Completed {
                return Ok(item);
            }
            if item.status == ItemStatus::Failed {
                return Err(last);
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(format!("Timed out waiting for owned job {id}: {last}"))
}

async fn clear_owned(clients: &Clients, s: &ClientSettings) -> Result<(), String> {
    for item in clients.items(s).await.map_err(|e| e.to_string())? {
        if item.title.starts_with("kitsunarr-client-proof") {
            clients
                .remove(s, &item, false)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn record(config: &Config, file: &str, rows: &[Value]) {
    std::fs::write(
        config.sandbox.join(file),
        serde_json::to_vec_pretty(rows).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
#[ignore = "requires five owned local containers and KITSUNARR_LIVE_CLIENT_CONFIG"]
async fn connection_authentication_and_paused_job_lifecycle() {
    let _lock = LIVE_LOCK.lock().await;
    let (config, proof, torrent, nzb) = load();
    let clients = Clients::new(reqwest::Client::new());
    let mut rows = Vec::new();
    for c in &config.clients {
        let s = settings(c, true);
        let outcome = async {
            let connected = clients.test(&s).await.map_err(|e| e.to_string())?;
            let status = clients.status(&s).await.map_err(|e| e.to_string())?;
            assert!(status.is_localhost);
            assert!(!status.output_root_folders.is_empty());
            let mut wrong = c.settings.clone();
            wrong.insert("password".into(), json!("deliberately-invalid-live-test-password"));
            wrong.insert("apiKey".into(), json!("deliberately-invalid-live-test-api-key"));
            let invalid = ClientSettings::from_provider(&c.implementation, &wrong).unwrap();
            if Clients::new(reqwest::Client::new()).test(&invalid).await.is_ok() {
                return Err(format!("{} accepted invalid credentials; check client authentication configuration",c.name));
            }
            clear_owned(&clients, &s).await?;
            purge_fixture_bytes(&c.downloads_path);
            let id = add(&clients, &s, &proof, &torrent, &nzb).await?;
            let mut queued = item(&clients, &s, &id, false).await?;
            for _ in 0..60 {
                if queued.status == ItemStatus::Paused { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
                queued = item(&clients, &s, &id, false).await?;
            }
            let state = queued.status;
            clients.remove(&s, &queued, false).await.map_err(|e| e.to_string())?;
            assert!(!clients.items(&s).await.map_err(|e|e.to_string())?.iter().any(|i|i.download_id.eq_ignore_ascii_case(&id)));
            if state != ItemStatus::Paused { return Err(format!("{} did not settle into paused state: {state:?}", c.name)); }
            Ok::<_, String>(json!({"client":c.implementation,"connection":connected.message,"warnings":connected.warnings,"invalidCredentialsRejected":true,"queueState":format!("{state:?}"),"remove":true}))
        }.await;
        match outcome {
            Ok(row) => {
                eprintln!(
                    "{}: connection, invalid auth, paused add, queue and removal passed",
                    c.implementation
                );
                rows.push(row);
            }
            Err(error) => {
                eprintln!("{}: failed", c.implementation);
                rows.push(json!({"client":c.implementation,"error":error}));
            }
        }
        record(&config, "lifecycle-results.json", &rows);
    }
    assert!(
        rows.iter().all(|r| r.get("error").is_none()),
        "live lifecycle failures: {rows:#?}"
    );
}

fn verified_file(dir: &Path, proof: &Proof) -> Option<PathBuf> {
    if dir.is_file() {
        let data = std::fs::read(dir).ok()?;
        return (data.len() as u64 == proof.bytes
            && hex::encode(Sha256::digest(&data)) == proof.sha256)
            .then(|| dir.to_path_buf());
    }
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let kind = entry.file_type().ok()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if let Some(found) = verified_file(&path, proof) {
                return Some(found);
            }
        } else if entry
            .file_name()
            .to_string_lossy()
            .starts_with("kitsunarr-client-proof")
            && path.extension().is_some_and(|e| e == "bin")
        {
            let data = std::fs::read(&path).ok()?;
            if data.len() as u64 == proof.bytes
                && hex::encode(Sha256::digest(&data)) == proof.sha256
            {
                return Some(path);
            }
        }
    }
    None
}

// Remove only this fixture's generated byte files, keeping client configuration/metadata.
fn purge_fixture_bytes(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let kind = entry.file_type().unwrap();
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            purge_fixture_bytes(&path);
        } else if entry
            .file_name()
            .to_string_lossy()
            .starts_with("kitsunarr-client-proof")
            && path.extension().is_some_and(|e| e == "bin")
        {
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires the local proof tracker/NNTP server and the five owned containers"]
async fn generated_local_data_downloads_and_integrity() {
    let _lock = LIVE_LOCK.lock().await;
    let (config, proof, torrent, nzb) = load();
    let clients = Clients::new(reqwest::Client::new());
    let seed = config
        .clients
        .iter()
        .find(|c| c.implementation == "Transmission")
        .unwrap();
    // Start Transmission first so it can also serve the other local torrent clients.
    let mut order = vec![seed];
    order.extend(
        config
            .clients
            .iter()
            .filter(|c| c.implementation != "Transmission"),
    );
    let mut rows = Vec::new();
    let mut completed = Vec::new();
    for c in order {
        let s = settings(c, false);
        let outcome = async {
            clear_owned(&clients,&s).await?;
            purge_fixture_bytes(&c.downloads_path);
            let id = add(&clients,&s,&proof,&torrent,&nzb).await?;
            let downloaded = item(&clients,&s,&id,true).await?;
            let output=downloaded.output_path.as_ref().ok_or_else(||format!("{} did not report its output path",c.name))?;
            let relative=Path::new(output).strip_prefix("/downloads").map_err(|e|e.to_string())?;
            let path = verified_file(&c.downloads_path.join(relative),&proof).ok_or_else(||format!("{} did not produce the exact generated bytes at {output}",c.name))?;
            if !s.is_usenet() {
                clients.mark_imported(&s,&id).await.map_err(|e|e.to_string())?;
                let mut imported = c.settings.clone();
                imported.insert("tvCategory".into(),json!("kitsunarr-client-imported"));
                let renamed = ClientSettings::from_provider(&c.implementation,&imported).unwrap();
                item(&clients,&renamed,&id,false).await?;
            }
            completed.push((s,downloaded));
            Ok::<_,String>(json!({"client":c.implementation,"completed":true,"bytes":proof.bytes,"sha256":proof.sha256,"path":path,"relabelAfterImport":!settings(c,false).is_usenet()}))
        }.await;
        match outcome {
            Ok(row) => {
                eprintln!(
                    "{}: local download completed and SHA-256 matched",
                    c.implementation
                );
                rows.push(row);
            }
            Err(error) => {
                eprintln!("{}: local transfer failed", c.implementation);
                rows.push(json!({"client":c.implementation,"error":error}));
            }
        }
        record(&config, "transfer-results.json", &rows);
    }
    for (settings, item) in completed {
        clients.remove(&settings, &item, false).await.unwrap();
    }
    assert!(
        rows.iter().all(|r| r.get("error").is_none()),
        "local transfer failures: {rows:#?}"
    );
}

#[tokio::test]
#[ignore = "requires an isolated Kitsunarr test API and owned local download clients"]
async fn native_api_observes_and_removes_each_owned_job() {
    let _lock = LIVE_LOCK.lock().await;
    let (config, proof, torrent, nzb) = load();
    let api = config
        .api
        .as_ref()
        .expect("configure the isolated test API");
    assert_eq!(api.base_url, "http://127.0.0.1:8991");
    let http = reqwest::Client::new();
    let clients = Clients::new(reqwest::Client::new());
    let endpoint = format!("{}/api/v1", api.base_url);
    let mut rows = Vec::new();
    let existing: Vec<Value> = http
        .get(format!("{endpoint}/providers/downloadclients"))
        .header("X-Api-Key", &api.api_key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    for c in &config.clients {
        let name = format!("Container test {}", c.implementation);
        if !existing.iter().any(|p| p["name"] == name) {
            http.post(format!("{endpoint}/providers/downloadclients")).header("X-Api-Key",&api.api_key)
                .json(&json!({"name":name,"implementation":c.implementation,"enabled":true,"settings":c.settings,"options":{"removeCompletedDownloads":false,"removeFailedDownloads":false}}))
                .send().await.unwrap().error_for_status().unwrap();
        }
        clear_owned(&clients, &settings(c, true)).await.unwrap();
        purge_fixture_bytes(&c.downloads_path);
    }
    for c in &config.clients {
        let s = settings(c, true);
        let id = add(&clients, &s, &proof, &torrent, &nzb).await.unwrap();
        let mut queued = item(&clients, &s, &id, false).await.unwrap();
        for _ in 0..60 {
            if queued.status == ItemStatus::Paused {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            queued = item(&clients, &s, &id, false).await.unwrap();
        }
        assert_eq!(queued.status, ItemStatus::Paused);
        let queue: Value = http
            .get(format!("{endpoint}/queue"))
            .header("X-Api-Key", &api.api_key)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(queue["failures"].as_array().unwrap().is_empty(), "{queue}");
        assert_eq!(queue["clients"], 5);
        let found = queue["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| {
                i["downloadId"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(&id))
            })
            .expect("owned job visible through native API");
        assert_eq!(found["status"], "paused");
        assert_eq!(
            found["downloadClient"],
            format!("Container test {}", c.implementation)
        );
        http.delete(format!(
            "{endpoint}/queue/{id}?removeFromClient=true&blocklist=false&skipRedownload=true"
        ))
        .header("X-Api-Key", &api.api_key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
        assert!(
            !clients
                .items(&s)
                .await
                .unwrap()
                .iter()
                .any(|i| i.download_id.eq_ignore_ascii_case(&id))
        );
        rows.push(json!({"client":c.implementation,"visibleThroughApi":true,"state":found["status"],"removedThroughApi":true}));
        eprintln!(
            "{}: visible and removed through native Kitsunarr queue API",
            c.implementation
        );
    }
    record(&config, "api-results.json", &rows);
}
