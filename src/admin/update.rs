use axum::{extract::State, Json};

use crate::{error::AppError, state::AppState, update};

use super::auth::AdminSession;

pub async fn check(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<update::UpdateInfo>, AppError> {
    update::check(&state)
        .await
        .map(Json)
        .map_err(|error| AppError::BadRequest(error.to_string()))
}

pub async fn apply(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<update::UpdateResult>, AppError> {
    update::apply(&state)
        .await
        .map(Json)
        .map_err(|error| AppError::BadRequest(error.to_string()))
}
