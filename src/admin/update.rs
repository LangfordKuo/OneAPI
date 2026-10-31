use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::{state::AppState, update};

use super::auth::AdminSession;

/// Translate an update failure into the JSON shape the admin UI shows. The
/// severity lets the UI pick a warning (recoverable: transport/metadata) or an
/// error (install failed: integrity, replace, storage) presentation, and the
/// hint names the next troubleshooting step.
fn failure_response(error: anyhow::Error) -> Response {
    let Some(failure) = error.downcast_ref::<update::UpdateError>() else {
        tracing::error!(%error, "unclassified update failure");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "message": error.to_string(),
                    "type": "update_error",
                    "severity": "error",
                    "kind": "internal",
                    "hint": "请查看服务端日志定位具体原因，或改用发布页手动下载安装包。",
                }
            })),
        )
            .into_response();
    };

    let status = match failure.kind {
        update::UpdateErrorKind::State => StatusCode::CONFLICT,
        update::UpdateErrorKind::Network => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    if failure.severity() == "warning" {
        tracing::warn!(kind = failure.kind.as_str(), error = %failure, "update failed");
    } else {
        tracing::error!(kind = failure.kind.as_str(), error = %failure, "update failed");
    }

    (
        status,
        Json(json!({
            "error": {
                "message": failure.message,
                "type": "update_error",
                "severity": failure.severity(),
                "kind": failure.kind.as_str(),
                "hint": failure.hint,
            }
        })),
    )
        .into_response()
}

pub async fn check(State(state): State<AppState>, _session: AdminSession) -> Response {
    match update::check(&state).await {
        Ok(info) => Json(info).into_response(),
        Err(error) => failure_response(error),
    }
}

pub async fn apply(State(state): State<AppState>, _session: AdminSession) -> Response {
    match update::apply(&state).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => failure_response(error),
    }
}
