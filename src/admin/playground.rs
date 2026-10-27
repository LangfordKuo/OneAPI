use axum::{extract::State, http::HeaderMap, response::Response, Json};
use serde_json::Value;

use crate::{
    error::AppError,
    gateway::execute::{self, ProxyIdentity},
    protocol::Protocol,
    state::AppState,
};

use super::auth::AdminSession;

pub async fn proxy(
    State(state): State<AppState>,
    _session: AdminSession,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Response, AppError> {
    let protocol = match body.get("protocol").and_then(Value::as_str) {
        Some("chat") => Protocol::Chat,
        Some("responses") => Protocol::Responses,
        Some("anthropic") => Protocol::Anthropic,
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unsupported playground protocol `{other}`"
            )))
        }
        None => return Err(AppError::BadRequest("protocol is required".to_string())),
    };

    let Some(object) = body.as_object_mut() else {
        return Err(AppError::BadRequest(
            "playground request must be a JSON object".to_string(),
        ));
    };
    object.remove("protocol");

    if protocol == Protocol::Responses {
        if object
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some()
        {
            return Err(AppError::BadRequest(
                "previous_response_id is not supported in the playground".to_string(),
            ));
        }
        object.insert("store".into(), Value::Bool(false));
    }

    execute::proxy_with_identity(
        &state,
        ProxyIdentity::playground(),
        protocol,
        &headers,
        body,
    )
    .await
}
