use axum::{
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use tower_http::{
    compression::{
        predicate::{DefaultPredicate, NotForContentType, Predicate},
        CompressionLayer,
    },
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::{admin, error::AppError, gateway, state::AppState, web};

pub fn router(state: AppState) -> Router {
    let request_id_header = HeaderName::from_static("x-request-id");

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/system", get(system_info))
        .route("/api/site", get(site_info))
        .merge(admin::routes(state.clone()))
        .merge(gateway::routes())
        .fallback(web::fallback)
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(
            CompressionLayer::new()
                .compress_when(DefaultPredicate::new().and(NotForContentType::SSE)),
        )
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Response headers applied to the admin UI, static assets and APIs. The CSP
/// allows the bundled SPA (external modules, inline style attributes used by
/// Radix) while blocking third-party scripts and framing.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; font-src 'self' data:; connect-src 'self'; \
             object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
        ),
    );
    response
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "name": "oneapi",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Serialize)]
struct ReadyStatus {
    status: &'static str,
    database_pool_size: u32,
    database_idle_connections: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_queue: Option<UsageQueueStatus>,
}

#[derive(Serialize)]
struct UsageQueueStatus {
    depth: usize,
    capacity: usize,
    sync_fallbacks: u64,
    failed_writes: u64,
    detail_queue_bytes: usize,
    detail_dropped: u64,
}

async fn readyz(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let _: i64 = sqlx::query_scalar("SELECT 1").fetch_one(&state.db).await?;
    let usage_queue = state.usage_writer.as_ref().map(|writer| {
        let snapshot = writer.snapshot();
        UsageQueueStatus {
            depth: snapshot.queue_depth,
            capacity: snapshot.queue_capacity,
            sync_fallbacks: snapshot.sync_fallbacks,
            failed_writes: snapshot.failed_writes,
            detail_queue_bytes: snapshot.detail_queue_bytes,
            detail_dropped: snapshot.detail_dropped,
        }
    });

    Ok(Json(ReadyStatus {
        status: "ready",
        database_pool_size: state.config.database_pool_size,
        database_idle_connections: state.db.num_idle(),
        usage_queue,
    }))
}

#[derive(Serialize)]
struct SiteInfo {
    site_name: String,
    version: &'static str,
}

/// Public branding endpoint used by the login/setup pages before a session exists.
async fn site_info(State(state): State<AppState>) -> Result<Json<SiteInfo>, AppError> {
    let site_name = state.cache.site_name(&state.db).await?;

    Ok(Json(SiteInfo {
        site_name,
        version: env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Serialize)]
struct SystemInfo {
    name: &'static str,
    version: &'static str,
    uptime_seconds: i64,
    database: &'static str,
    data_dir: String,
    database_bytes: u64,
    wal_bytes: u64,
    last_backup_at: Option<i64>,
    usage_queue_depth: usize,
    cooling_channels: usize,
    failovers_24h: i64,
}

async fn system_info(
    _session: crate::admin::AdminSession,
    State(state): State<AppState>,
) -> Result<Json<SystemInfo>, AppError> {
    let _: i64 = sqlx::query_scalar("SELECT 1").fetch_one(&state.db).await?;
    let uptime_seconds = (chrono::Utc::now() - state.started_at).num_seconds().max(0);

    let (database_bytes, wal_bytes) = if state.config.database_url.is_none() {
        crate::backup::database_sizes(&state.config.database_path)
    } else {
        (0, 0)
    };
    let last_backup_at = crate::backup::last_backup_at(&state.config.data_dir);
    let usage_queue_depth = state
        .usage_writer
        .as_ref()
        .map(|writer| writer.snapshot().queue_depth)
        .unwrap_or(0);

    let now = crate::models::now_ms();
    let health = state.cache.channel_health();
    let cooling_channels = state
        .cache
        .channels(&state.db)
        .await?
        .iter()
        .filter(|prepared| {
            // Same scope as the recovery task and `active_channels`: a channel
            // an operator disabled is not "cooling down", it is off.
            if prepared.channel.enabled == 0 {
                return false;
            }
            let disabled_until = health
                .get(&prepared.channel.id)
                .and_then(|health| health.disabled_until)
                .or(prepared.channel.disabled_until);
            disabled_until.is_some_and(|until| until > now)
        })
        .count();
    let failovers_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_logs WHERE created_at >= ? AND attempts > 1",
    )
    .bind(now - 86_400_000)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(SystemInfo {
        name: "OneAPI",
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds,
        database: "ok",
        data_dir: state.config.data_dir_display(),
        database_bytes,
        wal_bytes,
        last_backup_at,
        usage_queue_depth,
        cooling_channels,
        failovers_24h,
    }))
}
