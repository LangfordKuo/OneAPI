use axum::{extract::State, http::HeaderName, response::IntoResponse, routing::get, Json, Router};
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
        .merge(admin::routes(state.config.trust_proxy))
        .merge(gateway::routes())
        .fallback(web::fallback)
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(
            CompressionLayer::new()
                .compress_when(DefaultPredicate::new().and(NotForContentType::SSE)),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "name": "oneapi",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn readyz(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let _: i64 = sqlx::query_scalar("SELECT 1").fetch_one(&state.db).await?;
    Ok(Json(serde_json::json!({ "status": "ready" })))
}

#[derive(Serialize)]
struct SiteInfo {
    site_name: String,
    version: &'static str,
}

/// Public branding endpoint used by the login/setup pages before a session exists.
async fn site_info(State(state): State<AppState>) -> Result<Json<SiteInfo>, AppError> {
    let site_name = crate::models::setting::get(&state.db, "site_name")
        .await?
        .unwrap_or_else(|| "OneAPI".to_string());

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
}

async fn system_info(
    _session: crate::admin::AdminSession,
    State(state): State<AppState>,
) -> Result<Json<SystemInfo>, AppError> {
    let _: i64 = sqlx::query_scalar("SELECT 1").fetch_one(&state.db).await?;
    let uptime_seconds = (chrono::Utc::now() - state.started_at).num_seconds().max(0);

    Ok(Json(SystemInfo {
        name: "OneAPI",
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds,
        database: "ok",
        data_dir: state.config.data_dir_display(),
    }))
}
