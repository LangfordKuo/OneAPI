use axum::{extract::State, http::HeaderName, response::IntoResponse, routing::get, Json, Router};
use serde::Serialize;
use tower_http::{
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
        .merge(admin::routes())
        .merge(gateway::routes())
        .fallback(web::fallback)
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
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
struct SystemInfo {
    name: &'static str,
    version: &'static str,
    uptime_seconds: i64,
    database: &'static str,
    data_dir: String,
}

async fn system_info(State(state): State<AppState>) -> Result<Json<SystemInfo>, AppError> {
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
