//! Server-Sent Events stream of [`ServerEvent`](kitsunarr_domain::api::events::ServerEvent)s.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

/// Real-time event stream (`text/event-stream`). Events are named after `ServerEvent::name`;
/// a `resync` event tells the client it missed events (it lagged) and should refetch.
#[utoipa::path(get, path = "/events", tag = "system",
    responses((status = 200, description = "SSE stream of ServerEvent JSON payloads", content_type = "text/event-stream")))]
pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    // The receiver is dropped with the stream when the client disconnects.
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        let event = match rx.recv().await {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
                Event::default().event(event.name()).data(data)
            }
            Err(RecvError::Lagged(skipped)) => {
                tracing::debug!(skipped, "SSE client lagged");
                Event::default().event("resync").data("{}")
            }
            Err(RecvError::Closed) => return None,
        };
        Some((Ok(event), rx))
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(20))
            .text("keep-alive"),
    )
}
