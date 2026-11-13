//! Sonarr's SignalR hub (`/signalr/messages`) for tools that follow changes live (Bazarr):
//! the negotiate call and a WebSocket speaking SignalR's JSON hub protocol, sending Sonarr's
//! `receiveMessage` invocations (`{name, body: {action, resource}}`) for library, command and
//! health changes. Authentication is the v3 API's (`?access_token=` included).

use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::{get, post};
use kitsunarr_domain::api::events::ServerEvent;
use serde_json::{Value, json};

use crate::AppState;

/// SignalR's record separator.
const RS: char = '\u{1e}';

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/messages/negotiate", post(negotiate))
        .route("/messages", get(connect))
}

fn token() -> String {
    kitsunarr_auth::generate_api_key()
}

/// `POST /signalr/messages/negotiate`: WebSockets with text frames only.
async fn negotiate() -> Json<Value> {
    let id = token();
    Json(json!({
        "negotiateVersion": 1,
        "connectionId": id,
        "connectionToken": id,
        "availableTransports": [{"transport": "WebSockets", "transferFormats": ["Text"]}],
    }))
}

async fn connect(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| serve(state, socket))
}

/// Sonarr's messages for an event (`SignalRMessage`), as hub invocations.
pub fn messages(event: &ServerEvent) -> Vec<Value> {
    let invocation = |name: &str, body: Value| json!({"type": 1, "target": "receiveMessage", "arguments": [{"name": name, "body": body}]});
    match event {
        ServerEvent::LibraryChanged {
            media_type,
            ids,
            deleted,
        } if media_type == "series" => ids
            .iter()
            .map(|id| {
                invocation(
                    "series",
                    json!({"action": if *deleted { "deleted" } else { "updated" }, "resource": {"id": id}}),
                )
            })
            .collect(),
        ServerEvent::Task {
            run_id,
            task_name,
            status,
            message,
            ..
        } => vec![invocation(
            "command",
            json!({"action": "updated", "resource": {"id": run_id, "name": task_name, "commandName": task_name, "status": status, "message": message}}),
        )],
        ServerEvent::HealthChanged => vec![invocation("health", json!({"action": "sync"}))],
        _ => Vec::new(),
    }
}

fn frame(v: &Value) -> Message {
    Message::Text(format!("{v}{RS}").into())
}

async fn serve(state: AppState, mut socket: WebSocket) {
    // The handshake: `{"protocol":"json","version":1}` answered with `{}` (or an error).
    let handshake = match tokio::time::timeout(Duration::from_secs(15), socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => t.to_string(),
        _ => return,
    };
    let request: Value = serde_json::from_str(handshake.trim_end_matches(RS)).unwrap_or_default();
    if request["protocol"] != "json" {
        let _ = socket
            .send(frame(
                &json!({"error": "Only the JSON protocol is supported."}),
            ))
            .await;
        return;
    }
    if socket.send(frame(&json!({}))).await.is_err() {
        return;
    }
    let mut events = state.events.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(15));
    ping.tick().await;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    for m in messages(&event) {
                        if socket.send(frame(&m)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return,
            },
            _ = ping.tick() => {
                if socket.send(frame(&json!({"type": 6}))).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) => {
                    // A close message (type 7) ends the connection; pings need no answer.
                    if t.split(RS).filter(|p| !p.is_empty()).any(|p| {
                        serde_json::from_str::<Value>(p).is_ok_and(|v| v["type"] == 7)
                    }) {
                        return;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_become_sonarr_messages() {
        let m = messages(&ServerEvent::LibraryChanged {
            media_type: "series".into(),
            ids: vec![4, 5],
            deleted: false,
        });
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["target"], "receiveMessage");
        assert_eq!(m[0]["arguments"][0]["name"], "series");
        assert_eq!(m[1]["arguments"][0]["body"]["resource"]["id"], 5);
        assert_eq!(m[0]["arguments"][0]["body"]["action"], "updated");
        assert!(
            messages(&ServerEvent::LibraryChanged {
                media_type: "movie".into(),
                ids: vec![1],
                deleted: true
            })
            .is_empty()
        );
        assert_eq!(
            messages(&ServerEvent::HealthChanged)[0]["arguments"][0]["name"],
            "health"
        );
    }
}
