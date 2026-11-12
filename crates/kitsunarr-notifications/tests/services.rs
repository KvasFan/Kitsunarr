//! Each service against a recording mock: method, path, headers and body as upstream sends them.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::IntoResponse;
use kitsunarr_notifications::*;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: String,
    headers: HeaderMap,
    body: String,
}

type Log = Arc<Mutex<Vec<Seen>>>;

async fn record(
    State(log): State<Log>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let path = uri.path().to_string();
    log.lock().unwrap().push(Seen {
        method: method.to_string(),
        path: path.clone(),
        query: uri.query().unwrap_or_default().to_string(),
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    });
    if path == "/library/sections" {
        return axum::Json(json!({"MediaContainer": {"Directory": [
            {"key": "2", "type": "show", "Location": [{"path": "/media/tv"}]},
            {"key": "3", "type": "movie", "Location": [{"path": "/media/movies"}]}
        ]}}))
        .into_response();
    }
    axum::Json(json!({"ok": true, "result": "OK"})).into_response()
}

async fn mock() -> (String, u16, Log) {
    let log: Log = Arc::default();
    let app = axum::Router::new().fallback(record).with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://127.0.0.1:{port}"), port, log)
}

fn conn(implementation: &str, settings: Value) -> Connection {
    Connection {
        id: 1,
        name: implementation.into(),
        implementation: implementation.into(),
        triggers: json!({"onGrab": true, "onDownload": true, "onUpgrade": true})
            .as_object()
            .unwrap()
            .clone(),
        tags: vec![],
        settings: settings.as_object().cloned().unwrap_or_else(Map::new),
    }
}

fn series() -> SeriesRef {
    SeriesRef {
        id: 1,
        title: "Breaking Bad".into(),
        path: "/tv/Breaking Bad".into(),
        tvdb_id: 81189,
        series_type: "standard".into(),
        ..Default::default()
    }
}

fn download() -> Event {
    Event::Download(Box::new(Download {
        series: series(),
        episodes: vec![EpisodeRef {
            id: 1,
            season_number: 1,
            episode_number: 1,
            title: Some("Pilot".into()),
            series_id: 1,
            ..Default::default()
        }],
        file: FileRef {
            quality: "HDTV-720p".into(),
            quality_version: 1,
            relative_path: "Season 01/Breaking Bad - S01E01.mkv".into(),
            ..Default::default()
        },
        release: GrabbedRef::default(),
        deleted_files: vec![],
        download_client: Some("SABnzbd".into()),
        download_client_type: Some("Sabnzbd".into()),
        download_id: Some("x".into()),
    }))
}

fn instance() -> Instance {
    Instance {
        name: "Kitsunarr".into(),
        application_url: None,
    }
}

#[tokio::test]
async fn webhooks_send_sonarrs_payload_with_method_auth_and_headers() {
    let (base, _, log) = mock().await;
    let n = Notifier::default();
    let c = conn(
        "Webhook",
        json!({"url": format!("{base}/hook"), "method": 2, "username": "u", "password": "p",
               "headers": [{"key": "X-Custom", "value": "1"}]}),
    );
    n.send(&c, &download(), &instance()).await.unwrap();
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("PUT", "/hook"));
    assert_eq!(seen.headers["authorization"], "Basic dTpw");
    assert_eq!(seen.headers["x-custom"], "1");
    let body: Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["eventType"], "Download");
    assert_eq!(body["isUpgrade"], false);
    assert_eq!(body["episodeFile"]["quality"], "HDTV-720p");
}

#[tokio::test]
async fn chat_services_send_title_and_message() {
    let (base, port, log) = mock().await;
    let n = Notifier::default();
    let i = instance();
    let e = download();
    n.send(
        &conn(
            "Discord",
            json!({"webHookUrl": format!("{base}/discord"), "username": "K"}),
        ),
        &e,
        &i,
    )
    .await
    .unwrap();
    n.send(
        &conn(
            "Slack",
            json!({"webHookUrl": format!("{base}/slack"), "channel": "#tv"}),
        ),
        &e,
        &i,
    )
    .await
    .unwrap();
    n.send(
        &conn("Telegram", json!({"botToken": "123:abc", "chatId": "42", "apiUrl": base, "includeAppNameInTitle": true})),
        &e,
        &i,
    )
    .await
    .unwrap();
    n.send(
        &conn(
            "Gotify",
            json!({"server": base, "appToken": "tok", "priority": 7}),
        ),
        &e,
        &i,
    )
    .await
    .unwrap();
    n.send(
        &conn(
            "Ntfy",
            json!({"serverUrl": base, "topics": ["a", "b"], "priority": 4, "accessToken": "t"}),
        ),
        &e,
        &i,
    )
    .await
    .unwrap();
    n.send(
        &conn(
            "Apprise",
            json!({"serverUrl": base, "configurationKey": "kits", "notificationType": 1}),
        ),
        &e,
        &i,
    )
    .await
    .unwrap();
    n.send(
        &conn(
            "Signal",
            json!({"host": "127.0.0.1", "port": port, "senderNumber": "+1", "receiverId": "+2"}),
        ),
        &e,
        &i,
    )
    .await
    .unwrap();
    let seen = log.lock().unwrap().clone();
    let by_path = |p: &str| {
        seen.iter()
            .find(|s| s.path == p)
            .unwrap_or_else(|| panic!("{p}: {seen:#?}"))
    };
    let discord: Value = serde_json::from_str(&by_path("/discord").body).unwrap();
    assert_eq!(discord["username"], "K");
    assert_eq!(discord["embeds"][0]["description"], "Episode Imported");
    let slack: Value = serde_json::from_str(&by_path("/slack").body).unwrap();
    assert_eq!(slack["channel"], "#tv");
    assert_eq!(slack["attachments"][0]["title"], "Episode Downloaded");
    let telegram: Value = serde_json::from_str(&by_path("/bot123:abc/sendMessage").body).unwrap();
    assert_eq!(telegram["chat_id"], "42");
    assert_eq!(
        telegram["text"],
        "<b>Kitsunarr - Episode Downloaded</b>\nBreaking Bad - 1x01 - Pilot [HDTV-720p]"
    );
    let gotify = by_path("/message");
    assert_eq!(gotify.headers["x-gotify-key"], "tok");
    let gotify: Value = serde_json::from_str(&gotify.body).unwrap();
    assert_eq!(gotify["priority"], 7);
    let ntfy = by_path("/b");
    assert_eq!(ntfy.headers["x-title"], "Episode Downloaded");
    assert_eq!(ntfy.headers["x-priority"], "4");
    assert_eq!(ntfy.headers["authorization"], "Bearer t");
    assert!(seen.iter().any(|s| s.path == "/a"));
    let apprise: Value = serde_json::from_str(&by_path("/notify/kits").body).unwrap();
    assert_eq!(apprise["type"], "success");
    let signal: Value = serde_json::from_str(&by_path("/v2/send").body).unwrap();
    assert_eq!(signal["recipients"][0], "+2");
}

#[tokio::test]
async fn media_servers_refresh_the_series_folder() {
    let (_, port, log) = mock().await;
    let n = Notifier::default();
    let i = instance();
    let e = download();
    let emby = conn(
        "MediaBrowser",
        json!({"host": "127.0.0.1", "port": port, "apiKey": "k", "notify": true, "updateLibrary": true,
               "mapFrom": "/tv", "mapTo": "/media/tv"}),
    );
    n.send(&emby, &Event::Test, &i).await.unwrap();
    n.send(&emby, &e, &i).await.unwrap();
    let jellyfin = conn(
        "Jellyfin",
        json!({"host": "127.0.0.1", "port": port, "apiKey": "k", "refreshMode": 1,
               "tanukarrPathPrefix": "/tv", "jellyfinPathPrefix": "/srv/tv"}),
    );
    n.send(&jellyfin, &e, &i).await.unwrap();
    let plex = conn(
        "PlexServer",
        json!({"host": "127.0.0.1", "port": port, "authToken": "pt", "updateLibrary": true,
               "mapFrom": "/tv", "mapTo": "/media/tv"}),
    );
    n.send(&plex, &e, &i).await.unwrap();
    let kodi = conn(
        "Xbmc",
        json!({"host": "127.0.0.1", "port": port, "notify": true, "updateLibrary": true, "displayTime": 3}),
    );
    n.send(&kodi, &e, &i).await.unwrap();
    let seen = log.lock().unwrap().clone();
    let calls: Vec<String> = seen
        .iter()
        .map(|s| {
            format!(
                "{} {}{}",
                s.method,
                s.path,
                if s.query.is_empty() {
                    String::new()
                } else {
                    format!("?{}", s.query)
                }
            )
        })
        .collect();
    assert_eq!(
        calls,
        vec![
            "GET /System/Configuration",
            "POST /Notifications/Admin",
            "POST /Library/Media/Updated",
            "POST /Library/Media/Updated",
            "GET /library/sections",
            "GET /library/sections/2/refresh?path=%2Fmedia%2Ftv%2FBreaking+Bad",
            "POST /jsonrpc",
            "POST /jsonrpc",
        ]
    );
    assert_eq!(seen[0].headers["x-mediabrowser-token"], "k");
    let update: Value = serde_json::from_str(&seen[2].body).unwrap();
    assert_eq!(update["Updates"][0]["Path"], "/media/tv/Breaking Bad");
    let jellyfin: Value = serde_json::from_str(&seen[3].body).unwrap();
    assert_eq!(jellyfin["Updates"][0]["Path"], "/srv/tv/Breaking Bad");
    assert_eq!(seen[4].headers["x-plex-token"], "pt");
    let notify: Value = serde_json::from_str(&seen[6].body).unwrap();
    assert_eq!(notify["method"], "GUI.ShowNotification");
    assert_eq!(notify["params"]["displaytime"], 3000);
    let scan: Value = serde_json::from_str(&seen[7].body).unwrap();
    assert_eq!(scan["params"]["directory"], "/tv/Breaking Bad/");
}

#[tokio::test]
async fn dispatch_skips_unwanted_and_survives_failures() {
    let (base, _, log) = mock().await;
    let n = Notifier::default();
    let ok = conn("Webhook", json!({"url": format!("{base}/ok")}));
    let mut off = conn("Webhook", json!({"url": format!("{base}/off")}));
    off.triggers = json!({"onGrab": true}).as_object().unwrap().clone();
    let broken = conn("Webhook", json!({"url": "http://127.0.0.1:9/x"}));
    let unsupported = conn("Twitter", json!({}));
    let sent = n
        .dispatch(&[broken, off, ok, unsupported], &download(), &instance())
        .await;
    assert_eq!(sent, 1);
    let paths: Vec<String> = log.lock().unwrap().iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, vec!["/ok"]);
    assert!(!supported("Twitter"));
}

#[test]
fn custom_scripts_get_sonarrs_environment() {
    let env = kitsunarr_notifications::script::environment(&download(), &instance());
    assert_eq!(env["Sonarr_EventType"], "Download");
    assert_eq!(env["Sonarr_Series_Title"], "Breaking Bad");
    assert_eq!(env["Sonarr_Series_TvdbId"], "81189");
    assert_eq!(env["Sonarr_EpisodeFile_EpisodeNumbers"], "1");
    assert_eq!(env["Sonarr_EpisodeFile_Quality"], "HDTV-720p");
    assert_eq!(env["Sonarr_Download_Client"], "SABnzbd");
    assert_eq!(env["Sonarr_IsUpgrade"], "False");
    let rename = Event::Rename {
        series: Box::new(series()),
        files: vec![RenamedFile {
            id: 3,
            previous_relative_path: "a.mkv".into(),
            previous_path: "/tv/Breaking Bad/a.mkv".into(),
            relative_path: "Season 01/b.mkv".into(),
            path: "/tv/Breaking Bad/Season 01/b.mkv".into(),
        }],
    };
    let env = kitsunarr_notifications::script::environment(&rename, &instance());
    assert_eq!(env["Sonarr_EventType"], "Rename");
    assert_eq!(env["Sonarr_EpisodeFile_PreviousRelativePaths"], "a.mkv");
    assert_eq!(
        env["Sonarr_EpisodeFile_Paths"],
        "/tv/Breaking Bad/Season 01/b.mkv"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn custom_scripts_run_and_failures_are_reported() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let script = dir.path().join("hook.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$Sonarr_EventType $Sonarr_Series_Title $1\" > '{}'\n",
            out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let n = Notifier::default();
    n.send(
        &conn(
            "CustomScript",
            json!({"path": script.display().to_string(), "arguments": "extra"}),
        ),
        &download(),
        &instance(),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().trim(),
        "Download Breaking Bad extra"
    );
    let failing = dir.path().join("fail.sh");
    std::fs::write(&failing, "#!/bin/sh\necho broken >&2\nexit 3\n").unwrap();
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = n
        .send(
            &conn(
                "CustomScript",
                json!({"path": failing.display().to_string()}),
            ),
            &Event::Test,
            &instance(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("broken"), "{err}");
}

#[tokio::test]
async fn mailgun_and_sendgrid_mail_the_message() {
    let (base, _port, log) = mock().await;
    let n = Notifier::default();
    n.send(
        &conn("Mailgun", json!({"baseUrl": base, "apiKey": "mg-key", "senderDomain": "mg.example", "from": "k@mg.example", "recipients": ["me@example.org"]})),
        &download(),
        &instance(),
    )
    .await
    .unwrap();
    n.send(
        &conn("SendGrid", json!({"baseUrl": base, "apiKey": "sg-key", "from": "k@example.org", "recipients": ["me@example.org", "you@example.org"]})),
        &download(),
        &instance(),
    )
    .await
    .unwrap();
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen[0].path, "/v3/mg.example/messages");
    assert!(
        seen[0].body.contains("to=me%40example.org"),
        "{}",
        seen[0].body
    );
    assert!(
        seen[0].headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Basic ")
    );
    assert_eq!(seen[1].path, "/v3/mail/send");
    assert_eq!(seen[1].headers["authorization"], "Bearer sg-key");
    let body: Value = serde_json::from_str(&seen[1].body).unwrap();
    assert_eq!(
        body["personalizations"][0]["to"].as_array().unwrap().len(),
        2
    );
    assert_eq!(body["subject"], "Kitsunarr - Episode Downloaded");
}
