use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    models::{self, response::StoreOutcome},
    state::AppState,
};

/// Guard against unbounded `previous_response_id` chains: the expanded input is
/// sent upstream, so it needs its own ceiling independent of per-response size.
const MAX_EXPANDED_ITEMS: usize = 2_000;
const MAX_EXPANDED_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of prior turns walked for one request. Every link is an
/// indexed lookup, so a pathological chain must not turn into unbounded work.
const MAX_CHAIN_LINKS: usize = 256;
/// Maximum accepted length of a `previous_response_id`. The value is used as an
/// indexed lookup key, so it is validated before any database query.
pub const MAX_RESPONSE_CURSOR_LEN: usize = 128;

/// A response cursor is a bounded identifier: `[A-Za-z0-9_-]` only. Anything
/// else is rejected before the lookup so the raw value never reaches the query
/// or an error message.
fn valid_cursor(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_RESPONSE_CURSOR_LEN
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

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
        // Validate every cursor (the client's first one and each parent read
        // from storage) before it is used as an indexed lookup key.
        if !valid_cursor(&cursor) {
            return Err(ExpandError::Invalid(
                "previous_response_id is invalid".to_string(),
            ));
        }
        if chain.len() >= MAX_CHAIN_LINKS {
            return Err(ExpandError::Invalid(format!(
                "previous_response_id chain exceeds {MAX_CHAIN_LINKS} turns"
            )));
        }

        let stored = models::response::get(&state.db, &cursor, api_key_id)
            .await
            .map_err(ExpandError::Database)?
            .ok_or_else(|| {
                // Never echo the cursor: it is client-controlled and the
                // message may surface in logs or upstream error text.
                ExpandError::Invalid(
                    "previous_response_id was not found or was not stored".to_string(),
                )
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

/// Persist a Responses object. Returns `false` when the payload is too large,
/// exceeds the per-key/global storage quota, or has no id; callers must then
/// report `store: false` to the client instead of promising a retrievable
/// response.
pub async fn store(
    state: &AppState,
    api_key_id: i64,
    model: &str,
    request: &Value,
    response: &Value,
) -> sqlx::Result<bool> {
    let request_json = serde_json::to_string(request).unwrap_or_default();
    let response_json = serde_json::to_string(response).unwrap_or_default();

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

    let defaults = state.cache.defaults(&state.db).await?;
    let per_key_limit = storage_limit_bytes(defaults.response_storage_per_key_mb);
    let total_limit = storage_limit_bytes(defaults.response_storage_total_mb);

    // The quota check, upsert and counter update must observe one consistent
    // preimage; two concurrent requests must not both pass the same check.
    let _guard = state.response_store_lock.lock().await;

    let outcome = models::response::store_with_quota(
        &state.db,
        &id,
        api_key_id,
        model,
        &request_json,
        &response_json,
        &status,
        models::now_ms(),
        per_key_limit,
        total_limit,
    )
    .await?;

    match outcome {
        StoreOutcome::Stored => Ok(true),
        StoreOutcome::Conflict => {
            state
                .response_store_rejected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                response_id = %id,
                api_key_id,
                "response id is owned by another API key; skipping persistence"
            );
            Ok(false)
        }
        StoreOutcome::QuotaExceeded => {
            state
                .response_store_rejected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                response_id = %id,
                api_key_id,
                new_bytes = request_json.len().saturating_add(response_json.len()),
                per_key_limit,
                total_limit,
                "response storage quota exceeded; skipping persistence"
            );
            Ok(false)
        }
        StoreOutcome::TooLarge => {
            state
                .response_store_rejected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                response_id = %id,
                api_key_id,
                request_bytes = request_json.len(),
                response_bytes = response_json.len(),
                "response exceeds the storage limit; skipping persistence"
            );
            Ok(false)
        }
    }
}

/// Convert a MiB setting to bytes. `0` (or a negative hand-edited value) means
/// unlimited and stays `0`; the multiplication saturates so an out-of-range
/// stored value cannot wrap.
fn storage_limit_bytes(mb: i64) -> i64 {
    if mb <= 0 {
        0
    } else {
        mb.saturating_mul(1024 * 1024)
    }
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
