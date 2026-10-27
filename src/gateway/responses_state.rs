use serde_json::{json, Value};
use uuid::Uuid;

use crate::{models, state::AppState};

pub const MAX_STORED_BYTES: usize = 10 * 1024 * 1024;

pub enum ExpandError {
    Invalid(String),
    Database(sqlx::Error),
}

pub fn new_response_id() -> String {
    format!("resp_{}", Uuid::new_v4().simple())
}

pub async fn expand_previous(
    state: &AppState,
    api_key_id: i64,
    mut body: Value,
) -> Result<Value, ExpandError> {
    let Some(previous_id) = body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(body);
    };

    let stored = models::response::get(&state.db, &previous_id, api_key_id)
        .await
        .map_err(ExpandError::Database)?
        .ok_or_else(|| {
            ExpandError::Invalid(format!(
                "previous_response_id `{previous_id}` was not found or was not stored"
            ))
        })?;

    let previous_request: Value = serde_json::from_str(&stored.request_json)
        .map_err(|_| ExpandError::Invalid("stored previous request is invalid".to_string()))?;
    let previous_response: Value = serde_json::from_str(&stored.response_json)
        .map_err(|_| ExpandError::Invalid("stored previous response is invalid".to_string()))?;

    let mut input = normalize_input(previous_request.get("input"));
    if let Some(output) = previous_response.get("output").and_then(Value::as_array) {
        for item in output {
            if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                continue;
            }
            input.push(item.clone());
        }
    }
    input.extend(normalize_input(body.get("input")));

    let object = body
        .as_object_mut()
        .ok_or_else(|| ExpandError::Invalid("request must be a JSON object".to_string()))?;
    object.insert("input".into(), Value::Array(input));
    object.remove("previous_response_id");

    Ok(body)
}

fn normalize_input(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })],
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

pub async fn store(
    state: &AppState,
    api_key_id: i64,
    model: &str,
    request: &Value,
    response: &Value,
) -> sqlx::Result<()> {
    let request_json = serde_json::to_string(request).unwrap_or_default();
    let response_json = serde_json::to_string(response).unwrap_or_default();

    if request_json.len() > MAX_STORED_BYTES || response_json.len() > MAX_STORED_BYTES {
        tracing::warn!(
            request_bytes = request_json.len(),
            response_bytes = response_json.len(),
            "response exceeds the storage limit; skipping persistence"
        );
        return Ok(());
    }

    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        return Ok(());
    }

    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_string();

    models::response::insert(
        &state.db,
        &id,
        api_key_id,
        model,
        &request_json,
        &response_json,
        &status,
        models::now_ms(),
    )
    .await
}

pub async fn get(
    state: &AppState,
    api_key_id: i64,
    id: &str,
) -> sqlx::Result<Option<models::response::StoredResponse>> {
    models::response::get(&state.db, id, api_key_id).await
}

pub async fn delete(state: &AppState, api_key_id: i64, id: &str) -> sqlx::Result<bool> {
    models::response::delete(&state.db, id, api_key_id).await
}
