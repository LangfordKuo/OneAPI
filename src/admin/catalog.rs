use std::collections::BTreeSet;

use axum::{extract::State, Json};
use serde::Serialize;

use crate::{error::AppError, state::AppState};

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

    for prepared in channels.iter() {
        if prepared.channel.enabled == 0 {
            continue;
        }
        if prepared.catalog.has_wildcard {
            has_wildcard = true;
        }
        for model in &prepared.catalog.models {
            result.insert(model.clone());
        }
    }

    if has_wildcard {
        for model in state.cache.recent_models(&state.db).await? {
            if !model.is_empty() && model != "*" {
                result.insert(model);
            }
        }
    }

    Ok(Json(ModelsOutput {
        models: result.into_iter().collect(),
    }))
}
