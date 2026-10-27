use std::time::Instant;

use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    error::AppError,
    models::{self, channel::Channel, now_ms},
    protocol::{Protocol, Usage},
    state::AppState,
    upstream,
};

use super::{
    auth::ApiKeyAuth,
    error,
    responses_state::{self, ExpandError},
    select,
    stream::{self, ByteStream, StreamJob},
    usage::{self, UsageRecord},
};

#[derive(Debug, Clone, Copy)]
pub struct ProxyIdentity {
    pub api_key_id: Option<i64>,
    pub source: &'static str,
}

impl ProxyIdentity {
    pub fn gateway(api_key_id: i64) -> Self {
        Self {
            api_key_id: Some(api_key_id),
            source: "gateway",
        }
    }

    pub fn playground() -> Self {
        Self {
            api_key_id: None,
            source: "playground",
        }
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
        ProxyIdentity::gateway(auth.key.id),
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
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let defaults = models::setting::GatewayDefaults::load(&state.db).await?;
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
    let mut ctx = RequestContext {
        request_id: &request_id,
        api_key_id: identity.api_key_id,
        source: identity.source,
        client_model: &model,
        inbound,
        started,
        attempts: 0,
    };

    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let client_wants_usage = body
        .get("stream_options")
        .and_then(|value| value.get("include_usage"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

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

    let channels = models::channel::list(&state.db).await?;
    let candidates = select::candidates(channels, &model);
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
            client_wants_usage,
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

    for candidate in candidates {
        ctx.attempts += 1;
        let channel = &candidate.channel;
        let upstream = protocol_for_kind(&channel.kind);
        last_channel = Some(channel.clone());
        last_upstream_model = Some(candidate.upstream_model.clone());
        last_upstream = upstream;

        let request_body = if upstream == inbound {
            let mut value = body.clone();
            if let Some(object) = value.as_object_mut() {
                object.insert("model".into(), json!(candidate.upstream_model));
            }
            value
        } else {
            let mut ir = match inbound.parse_request(&body) {
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

        let client = match upstream::build_client(channel, defaults.nonstream_timeout_ms) {
            Ok(client) => client,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &last_message,
                    now_ms(),
                    60_000,
                )
                .await;
                continue;
            }
        };

        let request = match upstream::build_request(
            &client,
            channel,
            &state.crypto,
            &request_body,
            headers,
        ) {
            Ok(request) => request,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &last_message,
                    now_ms(),
                    60_000,
                )
                .await;
                continue;
            }
        };

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_unreachable".to_string();
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &last_message,
                    now_ms(),
                    60_000,
                )
                .await;
                continue;
            }
        };

        let status = response.status();
        let text = match response.text().await {
            Ok(text) => text,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_unreachable".to_string();
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &last_message,
                    now_ms(),
                    60_000,
                )
                .await;
                continue;
            }
        };
        let upstream_value = serde_json::from_str::<Value>(&text).ok();

        if status.is_success() {
            let _ = models::channel::mark_success(&state.db, channel.id).await;

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
                        let _ = models::channel::mark_failure(
                            &state.db,
                            channel.id,
                            message,
                            now_ms(),
                            60_000,
                        )
                        .await;
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
                    let _ = models::channel::mark_failure(
                        &state.db,
                        channel.id,
                        message,
                        now_ms(),
                        60_000,
                    )
                    .await;
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

                if should_store {
                    if let (Some(api_key_id), Some(request)) =
                        (identity.api_key_id, &client_request)
                    {
                        if let Err(error) =
                            responses_state::store(state, api_key_id, &model, request, &value).await
                        {
                            tracing::warn!(
                                error = %error,
                                request_id = %request_id,
                                "failed to persist Responses state"
                            );
                        }
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

            store_request_detail(state, &request_id, detail_request.as_deref(), &client_text).await;

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
            let _ =
                models::channel::mark_failure(&state.db, channel.id, &message, now_ms(), 60_000)
                    .await;
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

        store_request_detail(state, &request_id, detail_request.as_deref(), &text).await;

        return Ok(match upstream_value {
            Some(value) => error::upstream_error(inbound, status, &value),
            None => error::client_error(inbound, status, message, error_type.as_str()),
        });
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
    body: Value,
    candidates: Vec<select::Candidate>,
    should_store: bool,
    response_id: Option<String>,
    client_request: Option<Value>,
    started: Instant,
    client_wants_usage: bool,
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

    for candidate in candidates {
        attempts += 1;
        let channel = &candidate.channel;
        let upstream = protocol_for_kind(&channel.kind);
        last_channel = Some(channel.clone());
        last_upstream_model = Some(candidate.upstream_model.clone());
        last_upstream = upstream;

        let mut request_body = if upstream == inbound {
            let mut value = body.clone();
            if let Some(object) = value.as_object_mut() {
                object.insert("model".into(), json!(candidate.upstream_model));
            }
            value
        } else {
            let mut ir = match inbound.parse_request(&body) {
                Ok(ir) => ir,
                Err(error) => {
                    let message = error.to_string();
                    record_stream_attempt(
                        state,
                        request_id,
                        identity,
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
                        identity,
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

        if let Some(object) = request_body.as_object_mut() {
            object.insert("stream".into(), json!(true));
            if upstream == Protocol::Chat {
                let mut options = object
                    .get("stream_options")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                options.insert("include_usage".into(), json!(true));
                object.insert("stream_options".into(), Value::Object(options));
            }
            if upstream == Protocol::Responses {
                object.insert("store".into(), json!(false));
            }
        }

        let client = match upstream::build_stream_client(channel) {
            Ok(client) => client,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &last_message,
                    now_ms(),
                    60_000,
                )
                .await;
                continue;
            }
        };

        let request = match upstream::build_request(
            &client,
            channel,
            &state.crypto,
            &request_body,
            headers,
        ) {
            Ok(request) => request.header(header::ACCEPT, "text/event-stream"),
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &last_message,
                    now_ms(),
                    60_000,
                )
                .await;
                continue;
            }
        };

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_unreachable".to_string();
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &last_message,
                    now_ms(),
                    60_000,
                )
                .await;
                continue;
            }
        };

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let upstream_value = serde_json::from_str::<Value>(&text).ok();
            let (error_type, message) = upstream_error_info(upstream_value.as_ref(), &text);

            if is_retryable(status) {
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &message,
                    now_ms(),
                    60_000,
                )
                .await;
                last_error_type = error_type;
                last_message = message;
                continue;
            }

            record_stream_attempt(
                state,
                request_id,
                identity,
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
        }

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
            passthrough: upstream == inbound,
            client_wants_usage,
            store_request_detail: store_request_bodies,
            detail_request: detail_request.clone(),
            response_id: response_id.clone(),
            store_response: should_store,
            stored_request: client_request.clone(),
            stream: byte_stream,
        };

        match stream::start(job).await {
            Ok(mut response) => {
                add_route_headers(&mut response, channel, &candidate.upstream_model);
                return Ok(response);
            }
            Err(failure) => {
                let retryable = failure.retryable;
                let _ = models::channel::mark_failure(
                    &state.db,
                    channel.id,
                    &failure.message,
                    now_ms(),
                    60_000,
                )
                .await;
                last_error_type = failure.error_type.clone();
                last_message = failure.message.clone();

                if retryable {
                    continue;
                }

                record_stream_attempt(
                    state,
                    request_id,
                    identity,
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

    record_stream_attempt(
        state,
        request_id,
        identity,
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
    identity: ProxyIdentity,
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

    if let Err(error) = usage::insert(&state.db, &record).await {
        tracing::error!(
            error = %error,
            request_id = %request_id,
            "failed to write streaming usage log"
        );
    }
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

        if let Err(error) = usage::insert(&state.db, &record).await {
            tracing::error!(
                error = %error,
                request_id = self.request_id,
                "failed to write usage log"
            );
        }
    }
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
    request: Option<&str>,
    response: &str,
) {
    let Some(request) = request else {
        return;
    };
    if let Err(error) =
        models::request_detail::insert(&state.db, request_id, request, response, now_ms()).await
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
        return (error_type.to_string(), message.to_string());
    }

    (
        "upstream_error".to_string(),
        body.chars().take(300).collect(),
    )
}
