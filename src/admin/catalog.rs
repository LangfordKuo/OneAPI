use std::collections::BTreeSet;

use axum::{extract::State, Json};
use serde::Serialize;

use crate::{error::AppError, models, state::AppState};

use super::auth::AdminSession;

#[derive(Debug, Serialize)]
pub struct ModelsOutput {
    pub models: Vec<String>,
}

pub async fn list(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<ModelsOutput>, AppError> {
    let channels = models::channel::list(&state.db).await?;
    let mut result = BTreeSet::new();

    for channel in channels {
        if channel.enabled == 0 {
            continue;
        }
        let channel_models: Vec<String> = serde_json::from_str(&channel.models).unwrap_or_default();
        for model in channel_models {
            if !model.is_empty() && model != "*" {
                result.insert(model);
            }
        }
    }

    Ok(Json(ModelsOutput {
        models: result.into_iter().collect(),
    }))
}
