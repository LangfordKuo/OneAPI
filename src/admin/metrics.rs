use std::time::Duration;

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::Response,
};
use bytes::Bytes;
use futures_util::stream::unfold;

use crate::state::AppState;

use super::auth::AdminSession;

pub async fn stream(State(state): State<AppState>, _session: AdminSession) -> Response {
    let metrics = state.live.clone();
    let body = unfold(true, move |first| {
        let metrics = metrics.clone();
        async move {
            if !first {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let snapshot = metrics.snapshot();
            let data = serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string());
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
