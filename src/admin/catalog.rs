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
    let channels = state.cache.channels(&state.db).await?;
    let mut result = BTreeSet::new();
    let mut has_wildcard = false;

    for channel in channels {
        if channel.enabled == 0 {
            continue;
        }
        let channel_models: Vec<String> = serde_json::from_str(&channel.models).unwrap_or_default();
        for model in channel_models {
            if model == "*" {
                has_wildcard = true;
            } else if !model.is_empty() {
                result.insert(model);
            }
        }
    }

    if has_wildcard {
        let since = models::now_ms() - 30 * 24 * 60 * 60 * 1000;
        for model in models::usage_log::recent_models(&state.db, since, 200).await? {
            if !model.is_empty() && model != "*" {
                result.insert(model);
            }
        }
    }

    Ok(Json(ModelsOutput {
        models: result.into_iter().collect(),
    }))
}
