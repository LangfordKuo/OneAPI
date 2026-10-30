use std::time::Duration;

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::Response,
};
use bytes::Bytes;
use futures_util::stream::unfold;
use serde::Serialize;

use crate::{metrics::LiveSnapshot, state::AppState};

use super::auth::AdminSession;

/// Throughput snapshot plus a monotonic count of committed usage rows. It is
/// flattened into the same object the dashboard already consumes, so older
/// clients keep working: unknown fields are ignored and every frame remains a
/// valid metrics snapshot.
#[derive(Serialize)]
struct LiveFrame<'a> {
    #[serde(flatten)]
    snapshot: &'a LiveSnapshot,
    usage_total: u64,
}

pub async fn stream(State(state): State<AppState>, _session: AdminSession) -> Response {
    let metrics = state.live.clone();
    let events = state.events.clone();
    let body = unfold(true, move |first| {
        let metrics = metrics.clone();
        let events = events.clone();
        async move {
            if !first {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let snapshot = metrics.snapshot();
            let frame = LiveFrame {
                snapshot: &snapshot,
                usage_total: events.usage_rows(),
            };
            let data = serde_json::to_string(&frame).unwrap_or_else(|_| "{}".to_string());
            Some((
                Ok::<Bytes, std::io::Error>(Bytes::from(format!("data: {data}\n\n"))),
                false,
            ))
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body))
        .expect("valid SSE response")
}
