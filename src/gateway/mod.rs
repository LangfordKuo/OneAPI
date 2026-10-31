use std::collections::BTreeSet;

use axum::{
    extract::{DefaultBodyLimit, FromRequest, Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};

use crate::{error::AppError, models, protocol::Protocol, state::AppState, upstream};

use auth::ApiKeyAuth;

pub mod auth;
pub mod error;
pub mod execute;
pub mod responses_state;
pub mod select;
pub mod stream;
pub mod usage;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/responses", post(responses))
        .route(
            "/v1/responses/{id}",
            get(get_response).delete(delete_response),
        )
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
        .route("/v1/models", get(list_models))
        .route("/v1/models/{id}", get(get_model))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        // Explicit CORS: browsers may call `/v1` from any origin, but response
        // headers (`x-oneapi-*`, `x-request-id`, …) are deliberately not
        // exposed, so page scripts cannot read internal routing metadata.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

/// JSON extractor that maps malformed bodies to the protocol-specific error
/// shape instead of Axum's plain-text rejection.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let path = request.uri().path().to_string();
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(error::client_error(
                error::protocol_for_path(&path),
                rejection.status(),
                rejection.body_text(),
                "invalid_request_error",
            )),
        }
    }
}

async fn chat(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    ApiJson(body): ApiJson<Value>,
) -> Result<Response, AppError> {
    execute::proxy(&state, &auth, Protocol::Chat, &headers, body).await
}

async fn responses(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    ApiJson(body): ApiJson<Value>,
) -> Result<Response, AppError> {
    execute::proxy(&state, &auth, Protocol::Responses, &headers, body).await
}

async fn messages(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    ApiJson(body): ApiJson<Value>,
) -> Result<Response, AppError> {
    execute::proxy(&state, &auth, Protocol::Anthropic, &headers, body).await
}

/// Anthropic `count_tokens` endpoint. When an Anthropic upstream is available we
/// forward the request as-is; otherwise we return a clearly approximate estimate
/// so clients that probe context size keep working across providers.
async fn count_tokens(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    ApiJson(body): ApiJson<Value>,
) -> Result<Response, AppError> {
    if let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        if !auth.limits.allows_model(model) {
            return Ok(error::client_error(
                Protocol::Anthropic,
                StatusCode::FORBIDDEN,
                "model is not allowed for this API key",
                "permission_error",
            ));
        }

        let channels = state.cache.channels(&state.db).await?;
        let health = state.cache.channel_health();
        let allowed_channels = &auth.limits.channel_ids;
        let candidates: Vec<_> = select::candidates(
            &channels,
            &health,
            model,
            Protocol::Anthropic,
            allowed_channels,
        )
        .into_iter()
        .filter(|candidate| candidate.upstream_kind == "anthropic")
        .take(execute::MAX_FAILOVER_ATTEMPTS as usize)
        .collect();

        if !candidates.is_empty() {
            let defaults = state.cache.defaults(&state.db).await?;
            let mut client_error: Option<(StatusCode, String, String, bool)> = None;
            let mut last_message = "no upstream channel succeeded".to_string();
            let mut last_provider_json = false;

            for candidate in candidates {
                let mut request_body = body.clone();
                if let Some(object) = request_body.as_object_mut() {
                    object.insert("model".into(), json!(candidate.upstream_model));
                    object.remove("stream");
                }

                // Reuse the per-channel client so connection pools and TLS
                // sessions survive across count_tokens calls.
                let client = match state.clients.get(
                    candidate.channel(),
                    defaults.nonstream_timeout_ms,
                    false,
                ) {
                    Ok(client) => client,
                    Err(error) => {
                        last_message = error.to_string();
                        continue;
                    }
                };
                let url = format!(
                    "{}/messages/count_tokens",
                    candidate.upstream_config().base_url.trim_end_matches('/')
                );
                let request = match upstream::apply_auth(
                    client.post(url).json(&request_body),
                    candidate.upstream_config(),
                    &state.crypto,
                    "anthropic",
                    &headers,
                ) {
                    Ok(request) => request,
                    Err(error) => {
                        last_message = error.to_string();
                        continue;
                    }
                };

                let response = match request.send().await {
                    Ok(response) => response,
                    Err(error) => {
                        last_message = error.to_string();
                        continue;
                    }
                };
                let status = response.status();
                // Bound the buffered body the same way as every other
                // upstream read; a misbehaving upstream must not pin memory.
                let bytes = upstream::read_body_limited_bytes(response)
                    .await
                    .unwrap_or_default();
                if status.is_success() {
                    return Ok(
                        (status, [(header::CONTENT_TYPE, "application/json")], bytes)
                            .into_response(),
                    );
                }

                let text = String::from_utf8_lossy(&bytes);
                let value = serde_json::from_slice::<Value>(&bytes).ok();
                let (error_type, message) = execute::upstream_error_info(value.as_ref(), &text);
                let provider_json = value.is_some();
                // count_tokens is an auxiliary endpoint that not every
                // compatible provider implements, so failures here do not feed
                // channel health; they only drive failover and the final error.
                if !execute::penalizes_channel(status) && client_error.is_none() {
                    client_error = Some((
                        status,
                        error_type.clone(),
                        message.clone(),
                        provider_json,
                    ));
                }
                last_message = message;
                last_provider_json = provider_json;
            }

            return Ok(match client_error {
                Some((status, error_type, message, provider_json)) => {
                    let client_message = error::client_safe_upstream_message(
                        &error_type,
                        &message,
                        provider_json,
                    );
                    error::client_error(Protocol::Anthropic, status, client_message, &error_type)
                }
                None => error::client_error(
                    Protocol::Anthropic,
                    StatusCode::BAD_GATEWAY,
                    error::client_safe_upstream_message(
                        "upstream_error",
                        &last_message,
                        last_provider_json,
                    ),
                    "upstream_error",
                ),
            });
        }
    }

    Ok(Json(json!({ "input_tokens": usage::estimate_request_tokens(&body) })).into_response())
}

async fn get_response(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    match responses_state::get(&state, auth.key.id, &id).await? {
        Some(stored) => {
            let value: Value = serde_json::from_str(&stored.response_json)
                .map_err(|error| AppError::Internal(error.into()))?;
            Ok(Json(value).into_response())
        }
        None => Ok(error::client_error(
            Protocol::Responses,
            StatusCode::NOT_FOUND,
            "response not found",
            "not_found",
        )),
    }
}

async fn delete_response(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    if responses_state::delete(&state, auth.key.id, &id).await? {
        Ok(Json(json!({
            "id": id,
            "object": "response.deleted",
            "deleted": true,
        }))
        .into_response())
    } else {
        Ok(error::client_error(
            Protocol::Responses,
            StatusCode::NOT_FOUND,
            "response not found",
            "not_found",
        ))
    }
}

async fn list_models(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
) -> Result<Json<Value>, AppError> {
    let models = available_models(&state, &auth.limits, auth.key.id).await?;
    let first_id = models.first().cloned();
    let last_id = models.last().cloned();
    let data: Vec<Value> = models
        .into_iter()
        .map(|id| {
            json!({
                "id": id,
                "object": "model",
                "created": 0,
                "owned_by": "oneapi",
            })
        })
        .collect();

    Ok(Json(json!({
        "object": "list",
        "data": data,
        "has_more": false,
        "first_id": first_id,
        "last_id": last_id,
    })))
}

async fn get_model(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let models = available_models(&state, &auth.limits, auth.key.id).await?;
    if models.iter().any(|model| model == &id) {
        Ok(Json(json!({
            "id": id,
            "object": "model",
            "created": 0,
            "owned_by": "oneapi",
        }))
        .into_response())
    } else {
        Ok(error::client_error(
            Protocol::Chat,
            StatusCode::NOT_FOUND,
            "model not found",
            "not_found",
        ))
    }
}

async fn available_models(
    state: &AppState,
    limits: &models::api_key::ApiKeyLimits,
    key_id: i64,
) -> sqlx::Result<Vec<String>> {
    let channels = state.cache.channels(&state.db).await?;
    let mut result = BTreeSet::new();
    let mut has_wildcard = false;
    // The allowlists were already parsed once on the auth path.
    let allowed_channels = &limits.channel_ids;
    let channel_allowed =
        |channel_id: i64| allowed_channels.is_empty() || allowed_channels.contains(&channel_id);

    for prepared in channels.iter() {
        let channel = &prepared.channel;
        if channel.enabled == 0 || !channel_allowed(channel.id) {
            continue;
        }
        if prepared.catalog.has_wildcard {
            has_wildcard = true;
        }
        for model in &prepared.catalog.models {
            result.insert(model.clone());
        }
    }

    // Wildcard channels accept any model, so surface models seen in this key's
    // recent traffic to keep model pickers useful. The global cache is
    // deliberately not used here: it would leak every tenant's model names.
    if has_wildcard {
        let since = models::now_ms() - 30 * 24 * 60 * 60 * 1000;
        let key_models =
            models::usage_log::recent_models_for_key(&state.db, since, 200, key_id).await?;
        for model in key_models {
            if !model.is_empty() && model != "*" {
                result.insert(model);
            }
        }
    }

    // A per-key model allowlist narrows the catalog to models that can actually
    // be served by the channels this key may use.
    let allowed = &limits.models;
    if !allowed.is_empty() && !allowed.iter().any(|model| model == "*") {
        result.retain(|model| allowed.contains(model));
    }

    Ok(result.into_iter().collect())
}
