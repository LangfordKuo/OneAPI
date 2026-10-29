use serde_json::{json, Value};
use uuid::Uuid;

use crate::{models, state::AppState};

pub const MAX_STORED_BYTES: usize = 10 * 1024 * 1024;
/// Guard against unbounded `previous_response_id` chains: the expanded input is
/// sent upstream, so it needs its own ceiling independent of per-response size.
const MAX_EXPANDED_ITEMS: usize = 2_000;
const MAX_EXPANDED_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of prior turns walked for one request. Every link is an
/// indexed lookup, so a pathological chain must not turn into unbounded work.
const MAX_CHAIN_LINKS: usize = 256;

pub enum ExpandError {
    Invalid(String),
    Database(sqlx::Error),
}

pub fn new_response_id() -> String {
    format!("resp_{}", Uuid::new_v4().simple())
}

/// Rebuild the full conversation from the stored chain.
///
/// Each stored request holds only the client's own turn (plus the
/// `previous_response_id` that preceded it). Walking the chain backwards keeps
/// per-row storage O(1) instead of repeating the whole history on every turn.
/// Rows written by older versions contain the already-expanded history and no
/// `previous_response_id`, so they terminate the walk and still work.
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

    let mut chain: Vec<(Value, Value)> = Vec::new();
    let mut cursor = previous_id;
    let mut stored_bytes = 0usize;
    loop {
        if chain.len() >= MAX_CHAIN_LINKS {
            return Err(ExpandError::Invalid(format!(
                "previous_response_id chain exceeds {MAX_CHAIN_LINKS} turns"
            )));
        }

        let stored = models::response::get(&state.db, &cursor, api_key_id)
            .await
            .map_err(ExpandError::Database)?
            .ok_or_else(|| {
                ExpandError::Invalid(format!(
                    "previous_response_id `{cursor}` was not found or was not stored"
                ))
            })?;

        stored_bytes = stored_bytes
            .saturating_add(stored.request_json.len())
            .saturating_add(stored.response_json.len());
        if stored_bytes > MAX_EXPANDED_BYTES {
            return Err(ExpandError::Invalid(
                "stored conversation history exceeds the expanded size limit".to_string(),
            ));
        }

        let request: Value = serde_json::from_str(&stored.request_json)
            .map_err(|_| ExpandError::Invalid("stored previous request is invalid".to_string()))?;
        let response: Value = serde_json::from_str(&stored.response_json)
            .map_err(|_| ExpandError::Invalid("stored previous response is invalid".to_string()))?;
        let parent = request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::to_string);

        chain.push((request, response));
        match parent {
            Some(parent) => cursor = parent,
            None => break,
        }
    }

    let current_input = body.get("input").cloned();
    let current_bytes = serde_json::to_string(&current_input)
        .map(|value| value.len())
        .unwrap_or(0);
    if stored_bytes.saturating_add(current_bytes) > MAX_EXPANDED_BYTES {
        return Err(ExpandError::Invalid(
            "expanded conversation exceeds the size limit".to_string(),
        ));
    }

    let input = build_expanded_input(&chain, current_input.as_ref());
    if input.len() > MAX_EXPANDED_ITEMS {
        return Err(ExpandError::Invalid(format!(
            "expanded conversation has {} items; the limit is {MAX_EXPANDED_ITEMS}",
            input.len()
        )));
    }

    let object = body
        .as_object_mut()
        .ok_or_else(|| ExpandError::Invalid("request must be a JSON object".to_string()))?;
    object.insert("input".into(), Value::Array(input));
    object.remove("previous_response_id");

    Ok(body)
}

/// Concatenate stored turns (newest first, as loaded) and the current input in
/// chronological order, dropping reasoning items because they cannot be
/// replayed without an upstream id.
fn build_expanded_input(chain: &[(Value, Value)], current: Option<&Value>) -> Vec<Value> {
    let mut input = Vec::new();
    for (request, response) in chain.iter().rev() {
        input.extend(normalize_input(request.get("input")));
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for item in output {
                if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                    continue;
                }
                input.push(item.clone());
            }
        }
    }
    input.extend(normalize_input(current));
    input
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

/// Persist a Responses object. Returns `false` when the payload is too large or
/// has no id; callers must then report `store: false` to the client instead of
/// promising a retrievable response.
pub async fn store(
    state: &AppState,
    api_key_id: i64,
    model: &str,
    request: &Value,
    response: &Value,
) -> sqlx::Result<bool> {
    let request_json = serde_json::to_string(request).unwrap_or_default();
    let response_json = serde_json::to_string(response).unwrap_or_default();

    if request_json.len() > MAX_STORED_BYTES || response_json.len() > MAX_STORED_BYTES {
        tracing::warn!(
            request_bytes = request_json.len(),
            response_bytes = response_json.len(),
            "response exceeds the storage limit; skipping persistence"
        );
        return Ok(false);
    }

    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        return Ok(false);
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
    .await?;

    Ok(true)
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
