use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use bytes::Bytes;
use futures_util::stream::unfold;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::{watch, OwnedSemaphorePermit};

use crate::{
    error::AppError,
    events::EventBus,
    metrics::{LiveMetrics, LiveSnapshot},
    models,
    state::AppState,
};

use super::auth::{AdminSession, SESSION_COOKIE};

/// How often an established stream re-checks that its admin session still
/// exists. Logout and password changes delete the session row, so a stream
/// dies within this window even if the client keeps the socket open.
const SESSION_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const PUSH_INTERVAL: Duration = Duration::from_secs(1);

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

/// Everything a live stream needs across frames.
struct StreamState {
    /// Held for the stream's lifetime; dropping it frees the concurrency slot.
    _permit: OwnedSemaphorePermit,
    pushes: tokio::time::Interval,
    session_checks: tokio::time::Interval,
    shutdown: watch::Receiver<bool>,
    token_hash: String,
    db: SqlitePool,
    metrics: Arc<LiveMetrics>,
    events: Arc<EventBus>,
}

pub async fn stream(
    State(state): State<AppState>,
    jar: CookieJar,
    _session: AdminSession,
) -> Result<Response, AppError> {
    // Each accepted stream parks a permit until its body is dropped, so a
    // single client cannot exhaust sockets with reconnect storms or many tabs.
    let permit = state
        .metrics_streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::TooManyRequests("too many concurrent metrics streams".to_string())
        })?;

    // `AdminSession` already validated the token; the hash is kept so the
    // stream can re-validate the exact session it was opened with.
    let token_hash = jar
        .get(SESSION_COOKIE)
        .map(|cookie| crate::security::sha256_hex(cookie.value()))
        .unwrap_or_default();

    let mut pushes = tokio::time::interval(PUSH_INTERVAL);
    pushes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut session_checks = tokio::time::interval_at(
        tokio::time::Instant::now() + SESSION_CHECK_INTERVAL,
        SESSION_CHECK_INTERVAL,
    );
    session_checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let initial = StreamState {
        _permit: permit,
        pushes,
        session_checks,
        shutdown: state.shutdown.subscribe(),
        token_hash,
        db: state.db.clone(),
        metrics: state.live.clone(),
        events: state.events.clone(),
    };

    let body = unfold(initial, |state| async move {
        let StreamState {
            _permit,
            mut pushes,
            mut session_checks,
            mut shutdown,
            token_hash,
            db,
            metrics,
            events,
        } = state;

        // A stream that connects during shutdown must not start at all.
        let already_shutting_down = *shutdown.borrow();
        if already_shutting_down {
            return None;
        }

        loop {
            tokio::select! {
                // Ends the stream as soon as shutdown flips to true (or the
                // sender is gone), so it never blocks graceful shutdown.
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        return None;
                    }
                    let shutting_down = *shutdown.borrow();
                    if shutting_down {
                        return None;
                    }
                }
                _ = session_checks.tick() => {
                    match models::session::find_admin(&db, &token_hash, models::now_ms()).await {
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            // Logout, password change or expiry deleted the row.
                            tracing::debug!("closing metrics stream for an ended admin session");
                            return None;
                        }
                        Err(error) => {
                            // The session was valid when the stream opened; a
                            // transient database error must not kill dashboards.
                            tracing::warn!(
                                error = %error,
                                "failed to re-check metrics stream session"
                            );
                        }
                    }
                }
                _ = pushes.tick() => {
                    let snapshot = metrics.snapshot();
                    let frame = LiveFrame {
                        snapshot: &snapshot,
                        usage_total: events.usage_rows(),
                    };
                    let data = serde_json::to_string(&frame)
                        .unwrap_or_else(|_| "{}".to_string());
                    return Some((
                        Ok::<Bytes, std::io::Error>(Bytes::from(format!("data: {data}\n\n"))),
                        StreamState {
                            _permit,
                            pushes,
                            session_checks,
                            shutdown,
                            token_hash,
                            db,
                            metrics,
                            events,
                        },
                    ));
                }
            }
        }
    });

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body))
        .expect("valid SSE response"))
}
