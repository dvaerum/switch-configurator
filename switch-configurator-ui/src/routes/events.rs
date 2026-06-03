use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_core::Stream;
use std::convert::Infallible;

use super::AppState;

pub async fn sse_proxy(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let backend = state.backend.clone();

    let stream = async_stream::stream! {
        // Connect to backend SSE endpoint and forward events
        match backend.sse_stream("/api/events").await {
            Ok(mut rx) => {
                while let Some(event) = rx.recv().await {
                    yield Ok(Event::default()
                        .event(event.event_type)
                        .data(event.data));
                }
            }
            Err(e) => {
                tracing::error!("Failed to connect to backend SSE: {}", e);
                yield Ok(Event::default()
                    .event("error")
                    .data(format!("Backend SSE connection failed: {}", e)));
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
