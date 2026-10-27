use std::time::Instant;

use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
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
    error, select,
    usage::{self, UsageRecord},
};

pub async fn proxy(
    state: &AppState,
    auth: &ApiKeyAuth,
    inbound: Protocol,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, AppError> {
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return Ok(error::client_error(
            inbound,
            StatusCode::BAD_REQUEST,
            "model is required",
            "invalid_request_error",
        ));
    };

    let started = Instant::now();
    let mut ctx = RequestContext {
        request_id: &request_id,
        api_key_id: auth.key.id,
        client_model: &model,
        inbound,
        started,
        attempts: 0,
    };

    if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        let message = "streaming is not implemented yet";
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
        return Ok(error::client_error(
            inbound,
            StatusCode::BAD_REQUEST,
            message,
            "invalid_request_error",
        ));
    }

    if inbound == Protocol::Responses
        && body
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some()
    {
        let message = "previous_response_id is not implemented yet";
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
        return Ok(error::client_error(
            inbound,
            StatusCode::BAD_REQUEST,
            message,
            "invalid_request_error",
        ));
    }

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
        return Ok(error::client_error(
            inbound,
            StatusCode::NOT_FOUND,
            message,
            "invalid_request_error",
        ));
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

        let client = match upstream::build_client(channel) {
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

        let request = match upstream::build_request(&client, channel, &state.crypto, &request_body)
        {
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

            let (client_body, usage) = if upstream == inbound {
                let usage = upstream_value
                    .as_ref()
                    .and_then(|value| upstream.parse_response(value).ok())
                    .map(|response| response.usage)
                    .unwrap_or_default();
                (Body::from(text), usage)
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
                let bytes = serde_json::to_vec(&client_value)
                    .map_err(|error| AppError::Internal(error.into()))?;
                (Body::from(bytes), usage)
            };

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

            return Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(client_body)
                .map_err(|error| AppError::Internal(error.into()));
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

    Ok(error::client_error(
        inbound,
        StatusCode::BAD_GATEWAY,
        format!("all upstream channels failed: {last_message}"),
        &last_error_type,
    ))
}

struct RequestContext<'a> {
    request_id: &'a str,
    api_key_id: i64,
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
            source: "gateway",
            api_key_id: Some(self.api_key_id),
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
