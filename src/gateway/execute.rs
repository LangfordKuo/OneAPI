use std::time::{Duration, Instant};

use crate::{
    error::AppError,
    models::{self, api_key::ApiKey, channel::Channel, now_ms},
    protocol::{Protocol, UnifiedRequest, Usage},
    state::{AppState, ProbeAttempt},
    upstream,
};
use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    auth::ApiKeyAuth,
    error,
    responses_state::{self, ExpandError},
    select,
    stream::{self, ByteStream, StreamJob},
    usage::{self, UsageRecord},
};

/// Overall wall-clock budget for one client request, including failover.
const REQUEST_TOTAL_BUDGET: Duration = Duration::from_secs(600);
/// Time allowed to receive the first upstream SSE frame before committing.
const STREAM_COMMIT_BUDGET: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ProxyIdentity {
    pub api_key_id: Option<i64>,
    pub source: &'static str,
    allowed_channel_ids: Vec<i64>,
    allowed_models: Vec<String>,
}

impl ProxyIdentity {
    pub fn gateway(key: &ApiKey) -> Self {
        Self {
            api_key_id: Some(key.id),
            source: "gateway",
            allowed_channel_ids: key.channel_ids(),
            allowed_models: key.model_names(),
        }
    }

    pub fn playground() -> Self {
        Self {
            api_key_id: None,
            source: "playground",
            allowed_channel_ids: Vec::new(),
            allowed_models: Vec::new(),
        }
    }

    fn allows_model(&self, model: &str) -> bool {
        self.allowed_models.is_empty()
            || self
                .allowed_models
                .iter()
                .any(|item| item == "*" || item == model)
    }
}

pub async fn proxy(
    state: &AppState,
    auth: &ApiKeyAuth,
    inbound: Protocol,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, AppError> {
    proxy_with_identity(
        state,
        ProxyIdentity::gateway(&auth.key),
        inbound,
        headers,
        body,
    )
    .await
}

pub async fn proxy_with_identity(
    state: &AppState,
    identity: ProxyIdentity,
    inbound: Protocol,
    headers: &HeaderMap,
    mut body: Value,
) -> Result<Response, AppError> {
    let request_id = resolve_request_id(headers);
    let defaults = state.cache.defaults(&state.db).await?;
    let store_request_bodies = defaults.store_request_bodies;
    let detail_request =
        store_request_bodies.then(|| serde_json::to_string(&body).unwrap_or_default());

    let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        let message = "model is required";
        store_request_detail(
            state,
            &request_id,
            identity.api_key_id,
            detail_request.as_deref(),
            &error::client_error_value(inbound, message, "invalid_request_error").to_string(),
        )
        .await;
        return Ok(error::client_error(
            inbound,
            StatusCode::BAD_REQUEST,
            message,
            "invalid_request_error",
        ));
    };

    let started = Instant::now();
    let deadline = started + REQUEST_TOTAL_BUDGET;
    let mut ctx = RequestContext {
        request_id: &request_id,
        api_key_id: identity.api_key_id,
        source: identity.source,
        client_model: &model,
        inbound,
        started,
        attempts: 0,
    };

    if !identity.allows_model(&model) {
        let message = "model is not allowed for this API key";
        ctx.record(
            state,
            None,
            None,
            inbound,
            StatusCode::FORBIDDEN,
            Some(("permission_error", message)),
            Usage::default(),
        )
        .await;
        return Ok(error::client_error(
            inbound,
            StatusCode::FORBIDDEN,
            message,
            "permission_error",
        ));
    }

    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let client_wants_usage = body
        .get("stream_options")
        .and_then(|value| value.get("include_usage"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Local fallback used when a stream is interrupted before the final usage
    // frame arrives or the upstream never reports usage at all.
    let input_token_estimate = if streaming {
        usage::estimate_request_tokens(&body)
    } else {
        0
    };

    let mut client_request: Option<Value> = None;
    let mut response_id: Option<String> = None;
    let should_store = if inbound == Protocol::Responses {
        let requested_store = body.get("store").and_then(Value::as_bool).unwrap_or(true);
        let should_store = requested_store && identity.api_key_id.is_some();

        if let Some(api_key_id) = identity.api_key_id {
            body = match responses_state::expand_previous(state, api_key_id, body).await {
                Ok(value) => value,
                Err(ExpandError::Invalid(message)) => {
                    ctx.record(
                        state,
                        None,
                        None,
                        inbound,
                        StatusCode::BAD_REQUEST,
                        Some(("invalid_request_error", message.as_str())),
                        Usage::default(),
                    )
                    .await;
                    store_request_detail(
                        state,
                        &request_id,
                        identity.api_key_id,
                        detail_request.as_deref(),
                        &error::client_error_value(
                            inbound,
                            message.as_str(),
                            "invalid_request_error",
                        )
                        .to_string(),
                    )
                    .await;
                    return Ok(error::client_error(
                        inbound,
                        StatusCode::BAD_REQUEST,
                        message,
                        "invalid_request_error",
                    ));
                }
                Err(ExpandError::Database(error)) => return Err(AppError::Database(error)),
            };
        } else if body
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some()
        {
            let message = "previous_response_id is not supported in the playground";
            ctx.record(
                state,
                None,
                None,
                inbound,
                StatusCode::BAD_REQUEST,
                Some(("invalid_request_error", message)),
                Usage::default(),
            )
            .await;
            store_request_detail(
                state,
                &request_id,
                identity.api_key_id,
                detail_request.as_deref(),
                &error::client_error_value(inbound, message, "invalid_request_error").to_string(),
            )
            .await;
            return Ok(error::client_error(
                inbound,
                StatusCode::BAD_REQUEST,
                message,
                "invalid_request_error",
            ));
        }

        response_id = Some(responses_state::new_response_id());
        if let Some(object) = body.as_object_mut() {
            object.insert("store".into(), json!(false));
        }
        client_request = Some(body.clone());
        should_store
    } else {
        false
    };

    let channels = state.cache.channels(&state.db).await?;
    let candidates = select::candidates(&channels, &model, inbound, &identity.allowed_channel_ids);
    if candidates.is_empty() {
        let message = "no channel available for this model";
        ctx.record(
            state,
            None,
            None,
            inbound,
            StatusCode::NOT_FOUND,
            Some(("model_not_found", message)),
            Usage::default(),
        )
        .await;
        store_request_detail(
            state,
            &request_id,
            identity.api_key_id,
            detail_request.as_deref(),
            &error::client_error_value(inbound, message, "invalid_request_error").to_string(),
        )
        .await;
        return Ok(error::client_error(
            inbound,
            StatusCode::NOT_FOUND,
            message,
            "invalid_request_error",
        ));
    }

    if streaming {
        return proxy_stream(
            state,
            identity,
            inbound,
            &request_id,
            &model,
            body,
            candidates,
            should_store,
            response_id,
            client_request,
            started,
            deadline,
            client_wants_usage,
            input_token_estimate,
            store_request_bodies,
            detail_request.clone(),
            headers,
            defaults,
        )
        .await;
    }

    let mut last_channel: Option<Channel> = None;
    let mut last_upstream_model: Option<String> = None;
    let mut last_upstream = inbound;
    let mut last_error_type = "upstream_error".to_string();
    let mut last_message = "no upstream channel succeeded".to_string();
    let mut unified_request: Option<UnifiedRequest> = None;
    let mut probe_deferred = false;

    let last_candidate = candidates.len().saturating_sub(1);
    for (index, candidate) in candidates.into_iter().enumerate() {
        if Instant::now() >= deadline {
            last_error_type = "upstream_timeout".to_string();
            last_message = "request deadline exceeded".to_string();
            break;
        }
        let _probe = match state.probes.acquire(&candidate.channel, now_ms()) {
            ProbeAttempt::NotNeeded => None,
            ProbeAttempt::Acquired(lease) => Some(lease),
            ProbeAttempt::Busy => {
                probe_deferred = true;
                continue;
            }
        };
        ctx.attempts += 1;
        let channel = &candidate.channel;
        let upstream = protocol_for_kind(&candidate.upstream_kind);
        last_channel = Some(channel.clone());
        last_upstream_model = Some(candidate.upstream_model.clone());
        last_upstream = upstream;

        let request_body = if upstream == inbound {
            // The last attempt can consume the request body; earlier attempts
            // need a copy only because a later candidate may run.
            let mut value = if index == last_candidate {
                std::mem::take(&mut body)
            } else {
                body.clone()
            };
            if let Some(object) = value.as_object_mut() {
                object.insert("model".into(), json!(candidate.upstream_model));
            }
            value
        } else {
            if unified_request.is_none() {
                unified_request = Some(match inbound.parse_request(&body) {
                    Ok(ir) => ir,
                    Err(error) => {
                        let message = error.to_string();
                        ctx.record(
                            state,
                            Some(channel),
                            Some(&candidate.upstream_model),
                            upstream,
                            StatusCode::BAD_REQUEST,
                            Some(("invalid_request_error", message.as_str())),
                            Usage::default(),
                        )
                        .await;
                        return Ok(error::client_error(
                            inbound,
                            StatusCode::BAD_REQUEST,
                            message,
                            "invalid_request_error",
                        ));
                    }
                });
            }
            let mut ir = if index == last_candidate {
                unified_request.take().expect("parsed above")
            } else {
                unified_request.clone().expect("parsed above")
            };
            if upstream == Protocol::Anthropic && ir.max_tokens.is_none() {
                ir.max_tokens = Some(defaults.default_max_tokens);
            }
            ir.model = candidate.upstream_model.clone();
            match upstream.encode_request(&ir) {
                Ok(value) => value,
                Err(error) => {
                    let message = error.to_string();
                    ctx.record(
                        state,
                        Some(channel),
                        Some(&candidate.upstream_model),
                        upstream,
                        StatusCode::BAD_REQUEST,
                        Some(("invalid_request_error", message.as_str())),
                        Usage::default(),
                    )
                    .await;
                    return Ok(error::client_error(
                        inbound,
                        StatusCode::BAD_REQUEST,
                        message,
                        "invalid_request_error",
                    ));
                }
            }
        };

        let client = match state
            .clients
            .get(channel, defaults.nonstream_timeout_ms, false)
        {
            Ok(client) => client,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let request = match upstream::build_request(
            &client,
            &candidate.upstream_config,
            &state.crypto,
            &candidate.upstream_kind,
            &request_body,
            headers,
        ) {
            Ok(request) => request,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_unreachable".to_string();
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let status = response.status();
        let text = match upstream::read_body_limited(response).await {
            Ok(text) => text,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_unreachable".to_string();
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };
        let upstream_value = serde_json::from_str::<Value>(&text).ok();

        if status.is_success() {
            state.mark_channel_success(channel.id).await;

            let mut client_json: Option<Value> = None;
            let usage = if upstream == inbound {
                let usage = upstream_value
                    .as_ref()
                    .and_then(|value| upstream.parse_response(value).ok())
                    .map(|response| response.usage)
                    .unwrap_or_default();

                if inbound == Protocol::Responses {
                    client_json = upstream_value.clone();
                    if client_json.is_none() {
                        let message = "upstream returned a non-JSON response";
                        state.mark_channel_failure(channel.id, message).await;
                        ctx.record(
                            state,
                            Some(channel),
                            Some(&candidate.upstream_model),
                            upstream,
                            StatusCode::BAD_GATEWAY,
                            Some(("upstream_error", message)),
                            Usage::default(),
                        )
                        .await;
                        return Ok(error::client_error(
                            inbound,
                            StatusCode::BAD_GATEWAY,
                            message,
                            "upstream_error",
                        ));
                    }
                }

                usage
            } else {
                let Some(value) = upstream_value.as_ref() else {
                    let message = "upstream returned a non-JSON response";
                    state.mark_channel_failure(channel.id, message).await;
                    ctx.record(
                        state,
                        Some(channel),
                        Some(&candidate.upstream_model),
                        upstream,
                        StatusCode::BAD_GATEWAY,
                        Some(("upstream_error", message)),
                        Usage::default(),
                    )
                    .await;
                    return Ok(error::client_error(
                        inbound,
                        StatusCode::BAD_GATEWAY,
                        message,
                        "upstream_error",
                    ));
                };

                let mut unified = match upstream.parse_response(value) {
                    Ok(response) => response,
                    Err(error) => {
                        let message = error.to_string();
                        ctx.record(
                            state,
                            Some(channel),
                            Some(&candidate.upstream_model),
                            upstream,
                            StatusCode::BAD_GATEWAY,
                            Some(("upstream_error", message.as_str())),
                            Usage::default(),
                        )
                        .await;
                        return Ok(error::client_error(
                            inbound,
                            StatusCode::BAD_GATEWAY,
                            message,
                            "upstream_error",
                        ));
                    }
                };

                if unified.created_at == 0 {
                    unified.created_at = chrono::Utc::now().timestamp();
                }
                let usage = unified.usage.clone();
                let client_value = match inbound.encode_response(&unified) {
                    Ok(value) => value,
                    Err(error) => {
                        let message = error.to_string();
                        ctx.record(
                            state,
                            Some(channel),
                            Some(&candidate.upstream_model),
                            upstream,
                            StatusCode::BAD_GATEWAY,
                            Some(("conversion_error", message.as_str())),
                            Usage::default(),
                        )
                        .await;
                        return Ok(error::client_error(
                            inbound,
                            StatusCode::BAD_GATEWAY,
                            message,
                            "conversion_error",
                        ));
                    }
                };
                client_json = Some(client_value);
                usage
            };

            let (client_body, client_text) = if inbound == Protocol::Responses {
                let mut value = client_json.unwrap_or(Value::Null);
                if !value.is_object() {
                    let message = "failed to build a Responses response";
                    ctx.record(
                        state,
                        Some(channel),
                        Some(&candidate.upstream_model),
                        upstream,
                        StatusCode::BAD_GATEWAY,
                        Some(("upstream_error", message)),
                        Usage::default(),
                    )
                    .await;
                    store_request_detail(
                        state,
                        &request_id,
                        identity.api_key_id,
                        detail_request.as_deref(),
                        &error::client_error_value(inbound, message, "upstream_error").to_string(),
                    )
                    .await;
                    return Ok(error::client_error(
                        inbound,
                        StatusCode::BAD_GATEWAY,
                        message,
                        "upstream_error",
                    ));
                }

                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "id".into(),
                        json!(response_id
                            .as_ref()
                            .expect("responses request has a response id")),
                    );
                    object.insert("object".into(), json!("response"));
                    object.insert("store".into(), json!(should_store));
                    if !object.contains_key("created_at") {
                        object.insert("created_at".into(), json!(chrono::Utc::now().timestamp()));
                    }
                }

                let mut stored = false;
                if should_store {
                    if let (Some(api_key_id), Some(request)) =
                        (identity.api_key_id, &client_request)
                    {
                        match responses_state::store(state, api_key_id, &model, request, &value)
                            .await
                        {
                            Ok(persisted) => stored = persisted,
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    request_id = %request_id,
                                    "failed to persist Responses state"
                                );
                            }
                        }
                    }
                }
                // Never tell the client a response is retrievable when it was
                // skipped (for example because it exceeded the storage limit).
                if !stored {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("store".into(), json!(false));
                    }
                }

                let text = serde_json::to_string(&value)
                    .map_err(|error| AppError::Internal(error.into()))?;
                (Body::from(text.clone()), text)
            } else if let Some(value) = client_json {
                let text = serde_json::to_string(&value)
                    .map_err(|error| AppError::Internal(error.into()))?;
                (Body::from(text.clone()), text)
            } else {
                (Body::from(text.clone()), text)
            };

            // Some upstreams omit usage for converted or streaming-shaped
            // responses. Fall back to local estimates so the request is still
            // counted against the API key quota.
            let mut usage = usage;
            if usage.total_tokens.is_none()
                || usage.input_tokens.is_none()
                || usage.output_tokens.is_none()
            {
                usage.fill_missing(
                    usage::estimate_request_tokens(&request_body),
                    usage::estimate_text_tokens(&client_text),
                );
            }

            if let Some(output_tokens) = usage.output_tokens {
                state.live.add_tokens(Some(channel.id), output_tokens);
            }

            ctx.record(
                state,
                Some(channel),
                Some(&candidate.upstream_model),
                upstream,
                status,
                None,
                usage,
            )
            .await;

            store_request_detail(
                state,
                &request_id,
                identity.api_key_id,
                detail_request.as_deref(),
                &client_text,
            )
            .await;

            let response = Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(client_body)
                .map_err(|error| AppError::Internal(error.into()))?;
            return Ok(with_route_headers(
                response,
                channel,
                &candidate.upstream_model,
            ));
        }

        let (error_type, message) = upstream_error_info(upstream_value.as_ref(), &text);
        if is_retryable(status) {
            state.mark_channel_failure(channel.id, &message).await;
            last_error_type = error_type;
            last_message = message;
            continue;
        }

        ctx.record(
            state,
            Some(channel),
            Some(&candidate.upstream_model),
            upstream,
            status,
            Some((error_type.as_str(), message.as_str())),
            Usage::default(),
        )
        .await;

        store_request_detail(
            state,
            &request_id,
            identity.api_key_id,
            detail_request.as_deref(),
            &text,
        )
        .await;

        return Ok(match upstream_value {
            Some(value) => error::upstream_error(inbound, status, &value),
            None => error::client_error(inbound, status, message, error_type.as_str()),
        });
    }

    if ctx.attempts == 0 && probe_deferred {
        let message = "upstream channels are recovering; retry shortly";
        ctx.record(
            state,
            None,
            None,
            inbound,
            StatusCode::SERVICE_UNAVAILABLE,
            Some(("channel_recovering", message)),
            Usage::default(),
        )
        .await;
        store_request_detail(
            state,
            &request_id,
            identity.api_key_id,
            detail_request.as_deref(),
            &error::client_error_value(inbound, message, "channel_recovering").to_string(),
        )
        .await;
        return Ok(error::client_error(
            inbound,
            StatusCode::SERVICE_UNAVAILABLE,
            message,
            "channel_recovering",
        ));
    }

    ctx.record(
        state,
        last_channel.as_ref(),
        last_upstream_model.as_deref(),
        last_upstream,
        StatusCode::BAD_GATEWAY,
        Some((last_error_type.as_str(), last_message.as_str())),
        Usage::default(),
    )
    .await;

    let message = format!("all upstream channels failed: {last_message}");
    store_request_detail(
        state,
        &request_id,
        identity.api_key_id,
        detail_request.as_deref(),
        &error::client_error_value(inbound, &message, &last_error_type).to_string(),
    )
    .await;

    Ok(error::client_error(
        inbound,
        StatusCode::BAD_GATEWAY,
        message,
        &last_error_type,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn proxy_stream(
    state: &AppState,
    identity: ProxyIdentity,
    inbound: Protocol,
    request_id: &str,
    model: &str,
    mut body: Value,
    candidates: Vec<select::Candidate>,
    should_store: bool,
    response_id: Option<String>,
    client_request: Option<Value>,
    started: Instant,
    deadline: Instant,
    client_wants_usage: bool,
    input_token_estimate: u64,
    store_request_bodies: bool,
    detail_request: Option<String>,
    headers: &HeaderMap,
    defaults: models::setting::GatewayDefaults,
) -> Result<Response, AppError> {
    let mut attempts = 0u32;
    let mut last_channel: Option<Channel> = None;
    let mut last_upstream_model: Option<String> = None;
    let mut last_upstream = inbound;
    let mut last_error_type = "upstream_error".to_string();
    let mut last_message = "no upstream channel succeeded".to_string();
    let mut unified_request: Option<UnifiedRequest> = None;
    let mut probe_deferred = false;

    let last_candidate = candidates.len().saturating_sub(1);
    let commit_deadline = (Instant::now() + STREAM_COMMIT_BUDGET).min(deadline);
    'candidates: for (index, candidate) in candidates.into_iter().enumerate() {
        if Instant::now() >= deadline {
            last_error_type = "upstream_timeout".to_string();
            last_message = "request deadline exceeded".to_string();
            break;
        }
        let probe = match state.probes.acquire(&candidate.channel, now_ms()) {
            ProbeAttempt::NotNeeded => None,
            ProbeAttempt::Acquired(lease) => Some(lease),
            ProbeAttempt::Busy => {
                probe_deferred = true;
                continue;
            }
        };
        attempts += 1;
        let channel = &candidate.channel;
        let upstream = protocol_for_kind(&candidate.upstream_kind);
        last_channel = Some(channel.clone());
        last_upstream_model = Some(candidate.upstream_model.clone());
        last_upstream = upstream;

        let mut request_body = if upstream == inbound {
            let mut value = if index == last_candidate {
                std::mem::take(&mut body)
            } else {
                body.clone()
            };
            if let Some(object) = value.as_object_mut() {
                object.insert("model".into(), json!(candidate.upstream_model));
            }
            value
        } else {
            if unified_request.is_none() {
                unified_request = Some(match inbound.parse_request(&body) {
                    Ok(ir) => ir,
                    Err(error) => {
                        let message = error.to_string();
                        record_stream_attempt(
                            state,
                            request_id,
                            &identity,
                            model,
                            Some(channel),
                            Some(&candidate.upstream_model),
                            inbound,
                            upstream,
                            StatusCode::BAD_REQUEST,
                            attempts,
                            started,
                            Some(("invalid_request_error", message.as_str())),
                            Usage::default(),
                        )
                        .await;
                        return Ok(error::client_error(
                            inbound,
                            StatusCode::BAD_REQUEST,
                            message,
                            "invalid_request_error",
                        ));
                    }
                });
            }
            let mut ir = if index == last_candidate {
                unified_request.take().expect("parsed above")
            } else {
                unified_request.clone().expect("parsed above")
            };
            if upstream == Protocol::Anthropic && ir.max_tokens.is_none() {
                ir.max_tokens = Some(defaults.default_max_tokens);
            }
            ir.model = candidate.upstream_model.clone();
            match upstream.encode_request(&ir) {
                Ok(value) => value,
                Err(error) => {
                    let message = error.to_string();
                    record_stream_attempt(
                        state,
                        request_id,
                        &identity,
                        model,
                        Some(channel),
                        Some(&candidate.upstream_model),
                        inbound,
                        upstream,
                        StatusCode::BAD_REQUEST,
                        attempts,
                        started,
                        Some(("invalid_request_error", message.as_str())),
                        Usage::default(),
                    )
                    .await;
                    return Ok(error::client_error(
                        inbound,
                        StatusCode::BAD_REQUEST,
                        message,
                        "invalid_request_error",
                    ));
                }
            }
        };

        let mut include_usage_injected = false;
        if let Some(object) = request_body.as_object_mut() {
            object.insert("stream".into(), json!(true));
            if upstream == Protocol::Chat {
                let mut options = object
                    .get("stream_options")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                include_usage_injected =
                    options.get("include_usage").and_then(Value::as_bool) != Some(true);
                options.insert("include_usage".into(), json!(true));
                object.insert("stream_options".into(), Value::Object(options));
            }
            if upstream == Protocol::Responses {
                object.insert("store".into(), json!(false));
            }
        }

        let client = match state
            .clients
            .get(channel, defaults.nonstream_timeout_ms, true)
        {
            Ok(client) => client,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let mut usage_retry_done = false;
        let response = loop {
            let request = match upstream::build_request(
                &client,
                &candidate.upstream_config,
                &state.crypto,
                &candidate.upstream_kind,
                &request_body,
                headers,
            ) {
                Ok(request) => request.header(header::ACCEPT, "text/event-stream"),
                Err(error) => {
                    last_message = error.to_string();
                    last_error_type = "upstream_configuration_error".to_string();
                    state.mark_channel_failure(channel.id, &last_message).await;
                    continue 'candidates;
                }
            };

            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    last_message = error.to_string();
                    last_error_type = "upstream_unreachable".to_string();
                    state.mark_channel_failure(channel.id, &last_message).await;
                    continue 'candidates;
                }
            };

            let status = response.status();
            if status.is_success() {
                break response;
            }

            let text = upstream::read_body_limited(response)
                .await
                .unwrap_or_default();

            // Some OpenAI-compatible upstreams reject `stream_options` outright.
            // Retry once without the forced field so streaming still works; the
            // gateway then falls back to token estimation for usage accounting.
            if status == StatusCode::BAD_REQUEST
                && include_usage_injected
                && !usage_retry_done
                && rejects_stream_options(&text)
            {
                usage_retry_done = true;
                include_usage_injected = false;
                strip_include_usage(&mut request_body);
                continue;
            }

            let upstream_value = serde_json::from_str::<Value>(&text).ok();
            let (error_type, message) = upstream_error_info(upstream_value.as_ref(), &text);

            if is_retryable(status) {
                state.mark_channel_failure(channel.id, &message).await;
                last_error_type = error_type;
                last_message = message;
                continue 'candidates;
            }

            record_stream_attempt(
                state,
                request_id,
                &identity,
                model,
                Some(channel),
                Some(&candidate.upstream_model),
                inbound,
                upstream,
                status,
                attempts,
                started,
                Some((error_type.as_str(), message.as_str())),
                Usage::default(),
            )
            .await;

            return Ok(match upstream_value {
                Some(value) => error::upstream_error(inbound, status, &value),
                None => error::client_error(inbound, status, message, error_type.as_str()),
            });
        };

        let byte_stream: ByteStream = Box::pin(
            response
                .bytes_stream()
                .map(|item| item.map_err(std::io::Error::other)),
        );

        let job = StreamJob {
            state: state.clone(),
            request_id: request_id.to_string(),
            api_key_id: identity.api_key_id,
            source: identity.source,
            client_model: model.to_string(),
            inbound,
            upstream,
            channel_id: channel.id,
            upstream_model: candidate.upstream_model.clone(),
            started,
            attempts,
            idle_timeout: upstream::stream_idle_timeout(channel, defaults.stream_idle_timeout_ms),
            commit_deadline,
            passthrough: upstream == inbound,
            client_wants_usage,
            input_token_estimate,
            store_request_detail: store_request_bodies,
            detail_request: detail_request.clone(),
            response_id: response_id.clone(),
            store_response: should_store,
            stored_request: client_request.clone(),
            stream: byte_stream,
            _probe: probe,
        };

        match stream::start(job).await {
            Ok(mut response) => {
                add_route_headers(&mut response, channel, &candidate.upstream_model);
                return Ok(response);
            }
            Err(failure) => {
                let retryable = failure.retryable;
                state
                    .mark_channel_failure(channel.id, &failure.message)
                    .await;
                last_error_type = failure.error_type.clone();
                last_message = failure.message.clone();

                if retryable {
                    continue;
                }

                record_stream_attempt(
                    state,
                    request_id,
                    &identity,
                    model,
                    Some(channel),
                    Some(&candidate.upstream_model),
                    inbound,
                    upstream,
                    StatusCode::BAD_GATEWAY,
                    attempts,
                    started,
                    Some((failure.error_type.as_str(), failure.message.as_str())),
                    Usage::default(),
                )
                .await;

                return Ok(error::client_error(
                    inbound,
                    StatusCode::BAD_GATEWAY,
                    failure.message,
                    &failure.error_type,
                ));
            }
        }
    }

    if attempts == 0 && probe_deferred {
        let message = "upstream channels are recovering; retry shortly";
        record_stream_attempt(
            state,
            request_id,
            &identity,
            model,
            None,
            None,
            inbound,
            inbound,
            StatusCode::SERVICE_UNAVAILABLE,
            attempts,
            started,
            Some(("channel_recovering", message)),
            Usage::default(),
        )
        .await;
        store_request_detail(
            state,
            request_id,
            identity.api_key_id,
            detail_request.as_deref(),
            &error::client_error_value(inbound, message, "channel_recovering").to_string(),
        )
        .await;
        return Ok(error::client_error(
            inbound,
            StatusCode::SERVICE_UNAVAILABLE,
            message,
            "channel_recovering",
        ));
    }

    record_stream_attempt(
        state,
        request_id,
        &identity,
        model,
        last_channel.as_ref(),
        last_upstream_model.as_deref(),
        inbound,
        last_upstream,
        StatusCode::BAD_GATEWAY,
        attempts,
        started,
        Some((last_error_type.as_str(), last_message.as_str())),
        Usage::default(),
    )
    .await;

    let message = format!("all upstream channels failed: {last_message}");
    store_request_detail(
        state,
        request_id,
        identity.api_key_id,
        detail_request.as_deref(),
        &error::client_error_value(inbound, &message, &last_error_type).to_string(),
    )
    .await;

    Ok(error::client_error(
        inbound,
        StatusCode::BAD_GATEWAY,
        message,
        &last_error_type,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn record_stream_attempt(
    state: &AppState,
    request_id: &str,
    identity: &ProxyIdentity,
    model: &str,
    channel: Option<&Channel>,
    upstream_model: Option<&str>,
    inbound: Protocol,
    upstream: Protocol,
    status: StatusCode,
    attempts: u32,
    started: Instant,
    error: Option<(&str, &str)>,
    tokens: Usage,
) {
    let record = UsageRecord {
        request_id,
        source: identity.source,
        api_key_id: identity.api_key_id,
        channel_id: channel.map(|channel| channel.id),
        model,
        upstream_model,
        inbound_protocol: inbound.as_str(),
        upstream_protocol: upstream.as_str(),
        stream: true,
        status_code: status.as_u16(),
        attempts,
        error_type: error.map(|(error_type, _)| error_type),
        error_message: error.map(|(_, message)| message),
        usage: &tokens,
        latency_ms: started.elapsed().as_millis() as i64,
        first_token_ms: None,
        created_at: now_ms(),
    };

    usage::record(state, &record).await;
}

struct RequestContext<'a> {
    request_id: &'a str,
    api_key_id: Option<i64>,
    source: &'a str,
    client_model: &'a str,
    inbound: Protocol,
    started: Instant,
    attempts: u32,
}

impl RequestContext<'_> {
    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        state: &AppState,
        channel: Option<&Channel>,
        upstream_model: Option<&str>,
        upstream: Protocol,
        status: StatusCode,
        error: Option<(&str, &str)>,
        usage: Usage,
    ) {
        let record = UsageRecord {
            request_id: self.request_id,
            source: self.source,
            api_key_id: self.api_key_id,
            channel_id: channel.map(|channel| channel.id),
            model: self.client_model,
            upstream_model,
            inbound_protocol: self.inbound.as_str(),
            upstream_protocol: upstream.as_str(),
            stream: false,
            status_code: status.as_u16(),
            attempts: self.attempts,
            error_type: error.map(|(error_type, _)| error_type),
            error_message: error.map(|(_, message)| message),
            usage: &usage,
            latency_ms: self.started.elapsed().as_millis() as i64,
            first_token_ms: None,
            created_at: now_ms(),
        };

        usage::record(state, &record).await;
    }
}

const MAX_REQUEST_ID_LEN: usize = 128;

/// Accept a client-supplied request id only when it is short and uses a
/// conservative alphabet; otherwise generate one. The id is used as a log key
/// and as the storage key for request details, so it must not be attacker
/// controlled without bounds.
fn resolve_request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= MAX_REQUEST_ID_LEN
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
                })
        })
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn add_route_headers(response: &mut Response, channel: &Channel, upstream_model: &str) {
    use axum::http::HeaderValue;

    if let Ok(value) = HeaderValue::from_str(&channel.name) {
        response.headers_mut().insert("x-oneapi-channel", value);
    }
    if let Ok(value) = HeaderValue::from_str(upstream_model) {
        response
            .headers_mut()
            .insert("x-oneapi-upstream-model", value);
    }
}

fn with_route_headers(mut response: Response, channel: &Channel, upstream_model: &str) -> Response {
    add_route_headers(&mut response, channel, upstream_model);
    response
}

async fn store_request_detail(
    state: &AppState,
    request_id: &str,
    api_key_id: Option<i64>,
    request: Option<&str>,
    response: &str,
) {
    let Some(request) = request else {
        return;
    };
    if let Err(error) = models::request_detail::insert(
        &state.db,
        request_id,
        api_key_id.unwrap_or(0),
        request,
        response,
        now_ms(),
    )
    .await
    {
        tracing::warn!(
            error = %error,
            request_id = %request_id,
            "failed to store request detail"
        );
    }
}

fn protocol_for_kind(kind: &str) -> Protocol {
    match kind {
        "openai_responses" => Protocol::Responses,
        "anthropic" => Protocol::Anthropic,
        _ => Protocol::Chat,
    }
}

fn is_retryable(status: StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 404 | 408 | 409 | 425 | 429) || status.is_server_error()
}

fn upstream_error_info(value: Option<&Value>, body: &str) -> (String, String) {
    if let Some(value) = value {
        let error = value.get("error");
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .or_else(|| value.get("message").and_then(Value::as_str))
            .unwrap_or("upstream request failed");
        let error_type = error
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("upstream_error");
        return (
            error::truncate_message(error_type, 128),
            error::truncate_message(message, 1024),
        );
    }

    (
        "upstream_error".to_string(),
        body.chars().take(300).collect(),
    )
}

/// Detect upstream errors caused by the `stream_options`/`include_usage` fields
/// so streaming can be retried without them.
fn rejects_stream_options(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("stream_options") || lower.contains("include_usage")
}

fn strip_include_usage(body: &mut Value) {
    if let Some(object) = body.as_object_mut() {
        if let Some(options) = object
            .get_mut("stream_options")
            .and_then(Value::as_object_mut)
        {
            options.remove("include_usage");
            if options.is_empty() {
                object.remove("stream_options");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{rejects_stream_options, resolve_request_id, strip_include_usage};

    #[test]
    fn detects_include_usage_rejections() {
        assert!(rejects_stream_options(
            r#"{"error":{"message":"Unrecognized request argument supplied: stream_options"}}"#
        ));
        assert!(rejects_stream_options("include_usage is not supported"));
        assert!(!rejects_stream_options("invalid api key"));
    }

    #[test]
    fn strips_only_the_forced_include_usage_field() {
        let mut body = serde_json::json!({
            "stream": true,
            "stream_options": {"include_usage": true, "other": 1}
        });
        strip_include_usage(&mut body);
        assert_eq!(body["stream_options"], serde_json::json!({"other": 1}));

        let mut body = serde_json::json!({"stream_options": {"include_usage": true}});
        strip_include_usage(&mut body);
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn accepts_safe_client_request_ids() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req-abc_1.2:3"));
        assert_eq!(resolve_request_id(&headers), "req-abc_1.2:3");
    }

    #[test]
    fn replaces_unsafe_or_oversized_request_ids() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("contains a space"));
        assert_ne!(resolve_request_id(&headers), "contains a space");

        let oversized = "a".repeat(129);
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&oversized).expect("header"),
        );
        assert_ne!(resolve_request_id(&headers), oversized);

        headers.insert("x-request-id", HeaderValue::from_static(""));
        assert!(!resolve_request_id(&headers).is_empty());
    }
}
