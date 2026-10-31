use std::time::{Duration, Instant};

use crate::{
    error::AppError,
    models::{
        self,
        api_key::{ApiKey, ApiKeyLimits},
        channel::Channel,
        now_ms,
    },
    protocol::{self, Protocol, UnifiedRequest, Usage},
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
/// Maximum channels tried for one request. A personal deployment usually has
/// a handful of channels; beyond this point a retry is unlikely to help and
/// only multiplies latency and upstream calls.
pub(crate) const MAX_FAILOVER_ATTEMPTS: u32 = 3;
/// Wall-clock budget spent across failed attempts. It is checked only before
/// starting the next channel, so a single slow but successful response is not
/// cut short.
const FAILOVER_BUDGET: Duration = Duration::from_secs(60);

/// The first request-shaped failure seen while failing over. It is remembered
/// so that when every channel fails the client receives the upstream status
/// (for example 400) instead of a generic 502, together with the channel that
/// produced it for the usage log.
struct ClientFailure {
    status: StatusCode,
    error_type: String,
    /// Raw upstream text, kept for the usage log only.
    message: String,
    /// Whether `message` came from a structured provider JSON error. Only then
    /// may it be shown to the client (after redaction).
    provider_json: bool,
    channel_id: i64,
    upstream_model: String,
    upstream: Protocol,
}

impl ClientFailure {
    fn new(
        status: StatusCode,
        error_type: String,
        message: String,
        provider_json: bool,
        channel_id: i64,
        upstream_model: &str,
        upstream: Protocol,
    ) -> Self {
        Self {
            status,
            error_type,
            message,
            provider_json,
            channel_id,
            upstream_model: upstream_model.to_string(),
            upstream,
        }
    }

    /// The message this failure is allowed to expose to the API key client.
    fn client_message(&self) -> String {
        error::client_safe_upstream_message(&self.error_type, &self.message, self.provider_json)
    }
}

#[derive(Debug, Clone)]
pub struct ProxyIdentity {
    pub api_key_id: Option<i64>,
    pub source: &'static str,
    allowed_channel_ids: Vec<i64>,
    allowed_models: Vec<String>,
}

impl ProxyIdentity {
    pub fn gateway(key: &ApiKey, limits: &ApiKeyLimits) -> Self {
        Self {
            api_key_id: Some(key.id),
            source: "gateway",
            allowed_channel_ids: limits.channel_ids.clone(),
            allowed_models: limits.models.clone(),
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
        ProxyIdentity::gateway(&auth.key, &auth.limits),
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
    // Routing metadata (`x-oneapi-channel` / `x-oneapi-upstream-model`) is only
    // for the admin Playground UI; API key clients must not learn the internal
    // channel topology.
    let expose_route_headers = identity.source == "playground";

    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if let Some(model) = requested_model.as_deref() {
        // The model name is echoed into response headers and written to the
        // indexed `usage_logs.model` column. Reject over-long names and control
        // characters before fetching defaults, building the request detail,
        // selecting a channel or writing a usage row: this request never
        // reaches an upstream attempt, so it produces no usage row.
        if model.chars().count() > models::usage_log::MAX_MODEL_NAME_LEN
            || model.chars().any(char::is_control)
        {
            return Ok(error::client_error(
                inbound,
                StatusCode::BAD_REQUEST,
                "invalid model name",
                "invalid_request_error",
            ));
        }
    }

    let defaults = state.cache.defaults(&state.db).await?;
    let store_request_bodies = defaults.store_request_bodies;
    let detail_request =
        store_request_bodies.then(|| serde_json::to_string(&body).unwrap_or_default());

    let Some(model) = requested_model else {
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
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let mut log = AttemptLog {
        request_id: &request_id,
        api_key_id: identity.api_key_id,
        source: identity.source,
        client_model: &model,
        inbound,
        stream: streaming,
        started,
        reservation: None,
    };
    let mut attempts = 0u32;

    if !identity.allows_model(&model) {
        let message = "model is not allowed for this API key";
        log.record(
            state,
            None,
            None,
            inbound,
            StatusCode::FORBIDDEN,
            attempts,
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

        // Snapshot the client's own turn before expansion. Storing the expanded
        // history would make every row repeat the whole conversation (O(n²)
        // growth) and lose `previous_response_id`, which the chain walk needs.
        let client_snapshot = if should_store {
            Some(body.clone())
        } else {
            None
        };

        if let Some(api_key_id) = identity.api_key_id {
            body = match responses_state::expand_previous(state, api_key_id, body).await {
                Ok(value) => value,
                Err(ExpandError::Invalid(message)) => {
                    log.record(
                        state,
                        None,
                        None,
                        inbound,
                        StatusCode::BAD_REQUEST,
                        attempts,
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
            log.record(
                state,
                None,
                None,
                inbound,
                StatusCode::BAD_REQUEST,
                attempts,
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
        client_request = client_snapshot;
        should_store
    } else {
        false
    };

    // Admission estimate: the expanded body (Responses history included) plus
    // the worst-case completion size. Computed after `expand_previous` so the
    // reserved amount covers the history the upstream is actually asked to
    // process, not just the client's final turn.
    let input_estimate = usage::estimate_request_tokens(&body);
    // Local fallback used when a stream is interrupted before the final usage
    // frame arrives or the upstream never reports usage at all.
    let input_token_estimate = if streaming { input_estimate } else { 0 };
    let max_tokens = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .or_else(|| body.get("max_output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(defaults.default_max_tokens));
    let reservation_estimate =
        i64::try_from(input_estimate.saturating_add(max_tokens).max(1)).unwrap_or(i64::MAX);
    let reservation_day = now_ms() / 86_400_000;
    let mut reservation = match identity.api_key_id {
        Some(api_key_id) => {
            match usage::reserve(state, api_key_id, reservation_estimate, reservation_day).await {
                Ok(reservation) => reservation,
                Err(usage::ReserveError::Database(error)) => return Err(AppError::Database(error)),
                Err(refused) => return Ok(reserve_error_response(inbound, &refused)),
            }
        }
        None => None,
    };

    let channels = state.cache.channels(&state.db).await?;
    let health = state.cache.channel_health();
    let candidates = select::candidates(
        &channels,
        &health,
        &model,
        inbound,
        &identity.allowed_channel_ids,
    );
    if candidates.is_empty() {
        // A model whose channels are all in cooldown is not a missing model;
        // tell the client to retry instead of making it think routing changed.
        let cooling =
            select::has_matching_channel(&channels, &model, inbound, &identity.allowed_channel_ids);
        let (status, error_type, message) = if cooling {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "channel_recovering",
                "all channels for this model are cooling down; retry shortly",
            )
        } else {
            (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "no channel available for this model",
            )
        };
        log.record(
            state,
            None,
            None,
            inbound,
            status,
            attempts,
            Some((error_type, message)),
            Usage::default(),
        )
        .await;
        store_request_detail(
            state,
            &request_id,
            identity.api_key_id,
            detail_request.as_deref(),
            &error::client_error_value(inbound, message, error_type).to_string(),
        )
        .await;
        return Ok(error::client_error(inbound, status, message, error_type));
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
            reservation.take(),
            reservation_estimate,
            reservation_day,
        )
        .await;
    }

    // The non-stream path settles the reservation through its terminal usage
    // row (success or failover), so the log owns it from here on.
    log.reservation = reservation;

    let mut last_channel: Option<i64> = None;
    let mut last_upstream_model: Option<String> = None;
    let mut last_upstream = inbound;
    let mut last_error_type = "upstream_error".to_string();
    let mut last_message = "no upstream channel succeeded".to_string();
    let mut last_provider_json = false;
    // A request-shaped error (for example 400) is still worth retrying on
    // another channel, but it must not cool the channel down. Remember the
    // first one so the client gets a meaningful status if every channel fails.
    let mut client_error: Option<ClientFailure> = None;
    let mut unified_request: Option<UnifiedRequest> = None;
    let mut probe_deferred = false;

    let last_candidate = candidates.len().saturating_sub(1);
    for (index, candidate) in candidates.into_iter().enumerate() {
        if let Some((error_type, message)) = attempt_gate(attempts, started, deadline) {
            last_error_type = error_type.to_string();
            last_message = message.to_string();
            // Gateway-generated limit text is safe to forward; it contains no
            // upstream host information.
            last_provider_json = true;
            break;
        }
        let probe_lease = match state.probes.acquire(candidate.channel(), now_ms()) {
            ProbeAttempt::NotNeeded => None,
            ProbeAttempt::Acquired(lease) => Some(lease),
            ProbeAttempt::Busy => {
                probe_deferred = true;
                continue;
            }
        };
        attempts += 1;
        let channel = candidate.channel();
        let upstream = protocol_for_kind(&candidate.upstream_kind);
        last_channel = Some(channel.id);
        last_upstream_model = Some(candidate.upstream_model.clone());
        last_upstream = upstream;

        let request_body = match prepare_attempt_body(
            inbound,
            upstream,
            &mut body,
            &candidate,
            index == last_candidate,
            &mut unified_request,
            defaults.default_max_tokens,
        ) {
            AttemptBody::Ready(value) => value,
            AttemptBody::ConversionRejected(message) => {
                // The request is valid for the inbound protocol; it is this
                // target protocol that cannot represent it. Another channel may
                // use a different upstream protocol.
                if client_error.is_none() {
                    client_error = Some(ClientFailure::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error".to_string(),
                        message.clone(),
                        // A conversion rejection is gateway wording about the
                        // client payload, not raw upstream text, so it can be
                        // forwarded as-is.
                        true,
                        channel.id,
                        &candidate.upstream_model,
                        upstream,
                    ));
                }
                last_error_type = "invalid_request_error".to_string();
                last_message = message;
                last_provider_json = true;
                continue;
            }
            AttemptBody::InvalidRequest(message) => {
                log.record(
                    state,
                    Some(channel.id),
                    Some(&candidate.upstream_model),
                    upstream,
                    StatusCode::BAD_REQUEST,
                    attempts,
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

        let client = match state
            .clients
            .get(channel, defaults.nonstream_timeout_ms, false)
        {
            Ok(client) => client,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                last_provider_json = false;
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let request = match upstream::build_request(
            &client,
            candidate.upstream_config(),
            &state.crypto,
            &candidate.upstream_kind,
            &request_body,
            headers,
        ) {
            Ok(request) => request,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_configuration_error".to_string();
                last_provider_json = false;
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_unreachable".to_string();
                last_provider_json = false;
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let status = response.status();
        let body_bytes = match upstream::read_body_limited_bytes(response).await {
            Ok(bytes) => bytes,
            Err(error) => {
                last_message = error.to_string();
                last_error_type = "upstream_unreachable".to_string();
                last_provider_json = false;
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };
        let upstream_value = serde_json::from_slice::<Value>(&body_bytes).ok();

        if status.is_success() {
            if body_bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
                let message = "upstream returned an empty response";
                state.mark_channel_failure(channel.id, message).await;
                last_error_type = "upstream_error".to_string();
                last_message = message.to_string();
                last_provider_json = false;
                continue;
            }
            // Some compatible providers report failures inside a 200 body
            // (Responses uses a top-level `error` field when status=failed).
            // Treat it as a channel failure so another channel can answer.
            if let Some(value) = upstream_value.as_ref() {
                if value.get("error").is_some_and(|error| !error.is_null()) {
                    let (error_type, message) = upstream_error_info(Some(value), "");
                    state.mark_channel_failure(channel.id, &message).await;
                    last_error_type = error_type;
                    last_message = message;
                    last_provider_json = true;
                    continue;
                }
            }

            let mut client_json: Option<Value> = None;
            let usage = if upstream == inbound {
                // Only the usage block is needed here; converting the whole
                // response into the IR would copy every content part.
                let usage = upstream_value
                    .as_ref()
                    .map(|value| response_usage(upstream, value))
                    .unwrap_or_default();

                if inbound == Protocol::Responses {
                    client_json = upstream_value;
                    if client_json.is_none() {
                        let message = "upstream returned a non-JSON response";
                        state.mark_channel_failure(channel.id, message).await;
                        last_error_type = "upstream_error".to_string();
                        last_message = message.to_string();
                        last_provider_json = false;
                        continue;
                    }
                }

                usage
            } else {
                let Some(value) = upstream_value.as_ref() else {
                    let message = "upstream returned a non-JSON response";
                    state.mark_channel_failure(channel.id, message).await;
                    last_error_type = "upstream_error".to_string();
                    last_message = message.to_string();
                    last_provider_json = false;
                    continue;
                };

                let mut unified = match upstream.parse_response(value) {
                    Ok(response) => response,
                    Err(error) => {
                        let message = error.to_string();
                        state.mark_channel_failure(channel.id, &message).await;
                        last_error_type = "upstream_error".to_string();
                        last_message = message;
                        last_provider_json = false;
                        continue;
                    }
                };

                if unified.created_at == 0 {
                    unified.created_at = chrono::Utc::now().timestamp();
                }
                let usage = unified.usage.clone();
                let client_value = match inbound.encode_response(&unified) {
                    Ok(value) => value,
                    Err(error) => {
                        // A different channel may return a response this
                        // payload can be converted from; do not penalize the
                        // channel for a gateway-side conversion limit.
                        last_error_type = "conversion_error".to_string();
                        last_message = error.to_string();
                        // Gateway conversion wording is client-oriented and
                        // contains no upstream address, so keep it visible.
                        last_provider_json = true;
                        continue;
                    }
                };
                client_json = Some(client_value);
                usage
            };

            let (client_body, client_bytes) = if inbound == Protocol::Responses {
                let mut value = client_json.unwrap_or(Value::Null);
                if !value.is_object() {
                    let message = "failed to build a Responses response";
                    state.mark_channel_failure(channel.id, message).await;
                    last_error_type = "upstream_error".to_string();
                    last_message = message.to_string();
                    last_provider_json = false;
                    continue;
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

                let bytes = bytes::Bytes::from(
                    serde_json::to_vec(&value).map_err(|error| AppError::Internal(error.into()))?,
                );
                (Body::from(bytes.clone()), bytes)
            } else if let Some(value) = client_json {
                let bytes = bytes::Bytes::from(
                    serde_json::to_vec(&value).map_err(|error| AppError::Internal(error.into()))?,
                );
                (Body::from(bytes.clone()), bytes)
            } else {
                // Same protocol: forward the upstream bytes unchanged.
                (Body::from(body_bytes.clone()), body_bytes.clone())
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
                    usage::estimate_bytes_tokens(&client_bytes),
                );
            }

            if let Some(output_tokens) = usage.output_tokens {
                state.live.add_tokens(Some(channel.id), output_tokens);
            }

            // Mark success only after the payload is proven usable; a 2xx with
            // a malformed body above must not clear a channel's failure state.
            let may_have_failure = probe_lease.is_some()
                || channel.has_failure_state()
                || state.cache.channel_has_failure_override(channel.id);
            state
                .mark_channel_success_if_needed(channel.id, may_have_failure)
                .await;

            log.record(
                state,
                Some(channel.id),
                Some(&candidate.upstream_model),
                upstream,
                status,
                attempts,
                None,
                usage,
            )
            .await;

            if detail_request.is_some() {
                let response_text = String::from_utf8_lossy(&client_bytes);
                store_request_detail(
                    state,
                    &request_id,
                    identity.api_key_id,
                    detail_request.as_deref(),
                    &response_text,
                )
                .await;
            }

            let response = Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(client_body)
                .map_err(|error| AppError::Internal(error.into()))?;
            return Ok(if expose_route_headers {
                with_route_headers(response, channel, &candidate.upstream_model)
            } else {
                response
            });
        }

        let text = String::from_utf8_lossy(&body_bytes);
        let (error_type, message) = upstream_error_info(upstream_value.as_ref(), &text);
        let provider_json = upstream_value.is_some();
        if penalizes_channel(status) {
            state.mark_channel_failure(channel.id, &message).await;
        } else if client_error
            .as_ref()
            .is_some_and(|existing| existing.status == status && existing.error_type == error_type)
        {
            // The same request-shaped rejection from another channel means the
            // payload itself is the problem; retrying further only adds load.
            last_error_type = error_type;
            last_message = message;
            last_provider_json = provider_json;
            break;
        } else if client_error.is_none() {
            client_error = Some(ClientFailure::new(
                status,
                error_type.clone(),
                message.clone(),
                provider_json,
                channel.id,
                &candidate.upstream_model,
                upstream,
            ));
        }
        last_error_type = error_type;
        last_message = message;
        last_provider_json = provider_json;
    }

    finish_failover(
        state,
        &mut log,
        attempts,
        detail_request.as_deref(),
        FailoverState {
            last_channel,
            last_upstream_model,
            last_upstream,
            last_error_type,
            last_message,
            last_provider_json,
            client_error,
            probe_deferred,
        },
    )
    .await
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
    mut reservation: Option<usage::Reservation>,
    reservation_estimate: i64,
    reservation_day: i64,
) -> Result<Response, AppError> {
    // Only the admin Playground may see internal routing metadata.
    let expose_route_headers = identity.source == "playground";
    let mut log = AttemptLog {
        request_id,
        api_key_id: identity.api_key_id,
        source: identity.source,
        client_model: model,
        inbound,
        stream: true,
        started,
        reservation: None,
    };
    let mut attempts = 0u32;
    let mut last_channel: Option<i64> = None;
    let mut last_upstream_model: Option<String> = None;
    let mut last_upstream = inbound;
    let mut last_error_type = "upstream_error".to_string();
    let mut last_message = "no upstream channel succeeded".to_string();
    let mut last_provider_json = false;
    let mut client_error: Option<ClientFailure> = None;
    let mut unified_request: Option<UnifiedRequest> = None;
    let mut probe_deferred = false;

    let last_candidate = candidates.len().saturating_sub(1);
    'candidates: for (index, candidate) in candidates.into_iter().enumerate() {
        if let Some((error_type, message)) = attempt_gate(attempts, started, deadline) {
            last_error_type = error_type.to_string();
            last_message = message.to_string();
            // Gateway-generated limit text is safe to forward; it contains no
            // upstream host information.
            last_provider_json = true;
            break;
        }
        let probe = match state.probes.acquire(candidate.channel(), now_ms()) {
            ProbeAttempt::NotNeeded => None,
            ProbeAttempt::Acquired(lease) => Some(lease),
            ProbeAttempt::Busy => {
                probe_deferred = true;
                continue;
            }
        };
        attempts += 1;
        let channel = candidate.channel();
        let upstream = protocol_for_kind(&candidate.upstream_kind);
        last_channel = Some(channel.id);
        last_upstream_model = Some(candidate.upstream_model.clone());
        last_upstream = upstream;

        let mut request_body = match prepare_attempt_body(
            inbound,
            upstream,
            &mut body,
            &candidate,
            index == last_candidate,
            &mut unified_request,
            defaults.default_max_tokens,
        ) {
            AttemptBody::Ready(value) => value,
            AttemptBody::ConversionRejected(message) => {
                // The request is valid for the inbound protocol; it is this
                // target protocol that cannot represent it. Another channel may
                // use a different upstream protocol.
                if client_error.is_none() {
                    client_error = Some(ClientFailure::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error".to_string(),
                        message.clone(),
                        // A conversion rejection is gateway wording about the
                        // client payload, not raw upstream text, so it can be
                        // forwarded as-is.
                        true,
                        channel.id,
                        &candidate.upstream_model,
                        upstream,
                    ));
                }
                last_error_type = "invalid_request_error".to_string();
                last_message = message;
                last_provider_json = true;
                continue;
            }
            AttemptBody::InvalidRequest(message) => {
                log.record(
                    state,
                    Some(channel.id),
                    Some(&candidate.upstream_model),
                    upstream,
                    StatusCode::BAD_REQUEST,
                    attempts,
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
                last_provider_json = false;
                state.mark_channel_failure(channel.id, &last_message).await;
                continue;
            }
        };

        let mut usage_retry_done = false;
        let response = loop {
            let request = match upstream::build_request(
                &client,
                candidate.upstream_config(),
                &state.crypto,
                &candidate.upstream_kind,
                &request_body,
                headers,
            ) {
                Ok(request) => request.header(header::ACCEPT, "text/event-stream"),
                Err(error) => {
                    last_message = error.to_string();
                    last_error_type = "upstream_configuration_error".to_string();
                    last_provider_json = false;
                    state.mark_channel_failure(channel.id, &last_message).await;
                    continue 'candidates;
                }
            };

            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    last_message = error.to_string();
                    last_error_type = "upstream_unreachable".to_string();
                    last_provider_json = false;
                    state.mark_channel_failure(channel.id, &last_message).await;
                    continue 'candidates;
                }
            };

            let status = response.status();
            if status.is_success() {
                // A 200 with a non-SSE body (HTML error page, JSON payload) is
                // not a stream. Reject it before committing to this channel; an
                // absent header is left to the first-frame check.
                let invalid_content_type = response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .is_some_and(|value| {
                        !value
                            .to_str()
                            .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
                            .unwrap_or(false)
                    });
                if invalid_content_type {
                    // Drain the bounded body so the connection can be reused.
                    let _ = upstream::read_body_limited(response).await;
                    last_message = "upstream returned a non-SSE response".to_string();
                    last_error_type = "upstream_invalid_content_type".to_string();
                    last_provider_json = false;
                    state.mark_channel_failure(channel.id, &last_message).await;
                    continue 'candidates;
                }
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
            let provider_json = upstream_value.is_some();

            if penalizes_channel(status) {
                state.mark_channel_failure(channel.id, &message).await;
            } else if client_error.as_ref().is_some_and(|existing| {
                existing.status == status && existing.error_type == error_type
            }) {
                // Same rejection on another channel: stop retrying this payload.
                last_error_type = error_type;
                last_message = message;
                last_provider_json = provider_json;
                break 'candidates;
            } else if client_error.is_none() {
                client_error = Some(ClientFailure::new(
                    status,
                    error_type.clone(),
                    message.clone(),
                    provider_json,
                    channel.id,
                    &candidate.upstream_model,
                    upstream,
                ));
            }
            last_error_type = error_type;
            last_message = message;
            last_provider_json = provider_json;
            continue 'candidates;
        };

        let byte_stream: ByteStream = Box::pin(
            response
                .bytes_stream()
                .map(|item| item.map_err(std::io::Error::other)),
        );

        let clear_failure_state = probe.is_some()
            || channel.has_failure_state()
            || state.cache.channel_has_failure_override(channel.id);

        // Per-attempt budget: a retry that starts late still gets the full
        // window to receive its first frame, bounded only by the request
        // deadline shared by all attempts.
        let commit_deadline = (Instant::now() + STREAM_COMMIT_BUDGET).min(deadline);

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
            clear_failure_state,
            reservation: reservation.take(),
        };

        match stream::start(job).await {
            Ok(mut response) => {
                if expose_route_headers {
                    add_route_headers(&mut response, channel, &candidate.upstream_model);
                }
                return Ok(response);
            }
            Err(failure) => {
                state
                    .mark_channel_failure(channel.id, &failure.message)
                    .await;
                last_error_type = failure.error_type.clone();
                last_message = failure.message.clone();
                last_provider_json = failure.provider_json;

                // `start` consumed the job, so an armed guard was released when
                // the stream task exited before writing usage. Re-arm it for the
                // remaining candidates; if the quota is gone the request simply
                // continues unreserved (and will be rejected by its usage row).
                let rearm_key = if reservation.is_none() {
                    identity.api_key_id
                } else {
                    None
                };
                if let Some(api_key_id) = rearm_key {
                    match usage::reserve(state, api_key_id, reservation_estimate, reservation_day)
                        .await
                    {
                        Ok(value) => reservation = value,
                        Err(error) => tracing::warn!(
                            error = %error,
                            request_id,
                            "failed to re-reserve quota after a stream attempt failed"
                        ),
                    }
                }

                // Nothing was written downstream yet, so try the next channel.
                continue;
            }
        }
    }

    // No candidate committed: the failover row settles any reservation still
    // held here, exactly like a successful row would.
    log.reservation = reservation.take();

    finish_failover(
        state,
        &mut log,
        attempts,
        detail_request.as_deref(),
        FailoverState {
            last_channel,
            last_upstream_model,
            last_upstream,
            last_error_type,
            last_message,
            last_provider_json,
            client_error,
            probe_deferred,
        },
    )
    .await
}

/// One usage row for a single attempt, shared by the non-stream and stream
/// failover loops. The only difference between the two is the `stream` flag.
struct AttemptLog<'a> {
    request_id: &'a str,
    api_key_id: Option<i64>,
    source: &'a str,
    client_model: &'a str,
    inbound: Protocol,
    stream: bool,
    started: Instant,
    /// Admission reservation, settled by the usage row this log writes. Kept
    /// here so every return path (success or any failover outcome) releases it
    /// exactly once.
    reservation: Option<usage::Reservation>,
}

impl AttemptLog<'_> {
    #[allow(clippy::too_many_arguments)]
    async fn record(
        &mut self,
        state: &AppState,
        channel_id: Option<i64>,
        upstream_model: Option<&str>,
        upstream: Protocol,
        status: StatusCode,
        attempts: u32,
        error: Option<(&str, &str)>,
        usage: Usage,
    ) {
        let (reserved_tokens, reserved_day) = match &self.reservation {
            Some(reservation) => (reservation.tokens(), reservation.day()),
            None => (0, 0),
        };
        let record = UsageRecord {
            request_id: self.request_id,
            source: self.source,
            api_key_id: self.api_key_id,
            channel_id,
            model: self.client_model,
            upstream_model,
            inbound_protocol: self.inbound.as_str(),
            upstream_protocol: upstream.as_str(),
            stream: self.stream,
            status_code: status.as_u16(),
            attempts,
            error_type: error.map(|(error_type, _)| error_type),
            error_message: error.map(|(_, message)| message),
            usage: &usage,
            latency_ms: self.started.elapsed().as_millis() as i64,
            first_token_ms: None,
            reserved_tokens,
            reserved_day,
            created_at: now_ms(),
        };

        usage::record(state, &record).await;

        // The row now owns settlement; dropping it here would release a second
        // time from under the writer.
        if let Some(mut reservation) = self.reservation.take() {
            reservation.disarm();
        }
    }
}

/// Everything the two failover loops accumulate and then hand to
/// [`finish_failover`] when no attempt produced a usable payload.
struct FailoverState {
    last_channel: Option<i64>,
    last_upstream_model: Option<String>,
    last_upstream: Protocol,
    last_error_type: String,
    last_message: String,
    /// Whether `last_message` is structured provider JSON that may be shown to
    /// the client after redaction.
    last_provider_json: bool,
    client_error: Option<ClientFailure>,
    probe_deferred: bool,
}

/// Turn a fully exhausted failover attempt into the client response.
///
/// Shared by the non-stream and stream paths so both report the same status,
/// error shape and usage row: all channels cooling -> 503, a payload the
/// upstream rejected -> that upstream's status, otherwise 502.
async fn finish_failover(
    state: &AppState,
    log: &mut AttemptLog<'_>,
    attempts: u32,
    detail_request: Option<&str>,
    failover: FailoverState,
) -> Result<Response, AppError> {
    let inbound = log.inbound;

    if attempts == 0 && failover.probe_deferred {
        let message = "upstream channels are recovering; retry shortly";
        log.record(
            state,
            None,
            None,
            inbound,
            StatusCode::SERVICE_UNAVAILABLE,
            attempts,
            Some(("channel_recovering", message)),
            Usage::default(),
        )
        .await;
        store_request_detail(
            state,
            log.request_id,
            log.api_key_id,
            detail_request,
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

    // Every channel was tried. If one of them rejected the payload itself,
    // surface that status instead of hiding a client mistake behind a 502.
    if let Some(failure) = failover.client_error {
        let client_message = failure.client_message();
        log.record(
            state,
            Some(failure.channel_id),
            Some(&failure.upstream_model),
            failure.upstream,
            failure.status,
            attempts,
            Some((failure.error_type.as_str(), failure.message.as_str())),
            Usage::default(),
        )
        .await;
        store_request_detail(
            state,
            log.request_id,
            log.api_key_id,
            detail_request,
            // The raw upstream text is retained in the internal detail log.
            &error::client_error_value(inbound, &failure.message, &failure.error_type).to_string(),
        )
        .await;
        return Ok(error::client_error(
            inbound,
            failure.status,
            client_message,
            &failure.error_type,
        ));
    }

    log.record(
        state,
        failover.last_channel,
        failover.last_upstream_model.as_deref(),
        failover.last_upstream,
        StatusCode::BAD_GATEWAY,
        attempts,
        Some((
            failover.last_error_type.as_str(),
            failover.last_message.as_str(),
        )),
        Usage::default(),
    )
    .await;

    // The client only receives a safe classification; the full upstream text
    // stays in the usage row, the request detail and the channel health state.
    let client_message = error::client_safe_upstream_message(
        &failover.last_error_type,
        &failover.last_message,
        failover.last_provider_json,
    );
    let detail_message = format!("all upstream channels failed: {}", failover.last_message);
    store_request_detail(
        state,
        log.request_id,
        log.api_key_id,
        detail_request,
        &error::client_error_value(inbound, &detail_message, &failover.last_error_type).to_string(),
    )
    .await;

    Ok(error::client_error(
        inbound,
        StatusCode::BAD_GATEWAY,
        client_message,
        &failover.last_error_type,
    ))
}

/// Map a refused admission reservation to the protocol-shaped client error.
/// Status codes mirror the pre-existing auth checks: lifetime quota is 403,
/// daily quota is 429, and a key disabled between auth and admission is 401.
fn reserve_error_response(protocol: Protocol, cause: &usage::ReserveError) -> Response {
    match cause {
        usage::ReserveError::TokenLimit => error::client_error(
            protocol,
            StatusCode::FORBIDDEN,
            "API key token limit reached",
            "permission_error",
        ),
        usage::ReserveError::DailyLimit => error::client_error(
            protocol,
            StatusCode::TOO_MANY_REQUESTS,
            "API key daily token limit reached; it resets at 00:00 UTC",
            "rate_limit_error",
        ),
        usage::ReserveError::Disabled => error::client_error(
            protocol,
            StatusCode::UNAUTHORIZED,
            "invalid or missing API key",
            "authentication_error",
        ),
        // Callers convert the database case to `AppError` instead.
        usage::ReserveError::Database(_) => error::client_error(
            protocol,
            StatusCode::INTERNAL_SERVER_ERROR,
            "quota service unavailable",
            "api_error",
        ),
    }
}

/// Per-iteration limits shared by both failover loops. A `Some` value means the
/// loop must stop instead of starting another attempt.
fn attempt_gate(
    attempts: u32,
    started: Instant,
    deadline: Instant,
) -> Option<(&'static str, &'static str)> {
    if Instant::now() >= deadline {
        return Some(("upstream_timeout", "request deadline exceeded"));
    }
    if attempts >= MAX_FAILOVER_ATTEMPTS {
        return Some(("upstream_error", "failover attempt limit reached"));
    }
    if attempts > 0 && started.elapsed() >= FAILOVER_BUDGET {
        return Some(("upstream_timeout", "failover time budget exceeded"));
    }
    None
}

/// Result of building one attempt's upstream body.
enum AttemptBody {
    Ready(Value),
    /// The inbound payload is valid but this target protocol cannot express it.
    /// Another channel may speak a different protocol, so keep failing over
    /// without cooling the channel down.
    ConversionRejected(String),
    /// The inbound payload itself is invalid: no channel can accept it.
    InvalidRequest(String),
}

/// Build the upstream body for one attempt, either by forwarding the inbound
/// JSON (same protocol) or by converting it through the IR.
fn prepare_attempt_body(
    inbound: Protocol,
    upstream: Protocol,
    body: &mut Value,
    candidate: &select::Candidate,
    last_candidate: bool,
    unified_request: &mut Option<UnifiedRequest>,
    default_max_tokens: u32,
) -> AttemptBody {
    if upstream == inbound {
        // The last attempt can consume the request body; earlier attempts need
        // a copy only because a later candidate may run.
        let mut value = if last_candidate {
            std::mem::take(body)
        } else {
            body.clone()
        };
        if let Some(object) = value.as_object_mut() {
            object.insert("model".into(), json!(candidate.upstream_model));
        }
        return AttemptBody::Ready(value);
    }

    if unified_request.is_none() {
        *unified_request = Some(match inbound.parse_request(body) {
            Ok(ir) => ir,
            Err(error) => return AttemptBody::InvalidRequest(error.to_string()),
        });
    }
    let mut ir = if last_candidate {
        unified_request.take().expect("parsed above")
    } else {
        unified_request.clone().expect("parsed above")
    };
    if upstream == Protocol::Anthropic && ir.max_tokens.is_none() {
        ir.max_tokens = Some(default_max_tokens);
    }
    ir.model = candidate.upstream_model.clone();
    match upstream.encode_request(&ir) {
        Ok(value) => AttemptBody::Ready(value),
        Err(error) => AttemptBody::ConversionRejected(error.to_string()),
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
    // Truncate before building the record so an oversized body is never copied
    // around in full; `OwnedDetailRecord` re-applies the same bound.
    let request = models::request_detail::truncate_detail(request);
    let response = models::request_detail::truncate_detail(response);
    usage::record_detail(
        state,
        &usage::DetailRecord {
            request_id,
            api_key_id: api_key_id.unwrap_or(0),
            request_body: &request,
            response_body: &response,
            created_at: now_ms(),
        },
    )
    .await;
}

fn protocol_for_kind(kind: &str) -> Protocol {
    match kind {
        "openai_responses" => Protocol::Responses,
        "anthropic" => Protocol::Anthropic,
        _ => Protocol::Chat,
    }
}

/// Read the usage block of a same-protocol response. This deliberately avoids
/// `parse_response`, which would rebuild the whole payload in the IR just to
/// obtain the token counters.
fn response_usage(protocol: Protocol, value: &Value) -> Usage {
    let Some(usage) = value.get("usage").and_then(Value::as_object) else {
        return Usage::default();
    };
    match protocol {
        Protocol::Chat => crate::protocol::chat::parse_usage(usage),
        Protocol::Responses => crate::protocol::responses::parse_usage(usage),
        Protocol::Anthropic => crate::protocol::anthropic::parse_usage(usage),
    }
}

/// Whether an upstream failure says something about the channel's health.
///
/// Failover itself no longer depends on the status code: the same model may
/// work on another channel, so every failure is retried on the next candidate.
/// Only failures that point at the channel (auth, quota, rate limit, missing
/// model, timeout, server error) feed the cooldown. Request-shaped errors such
/// as 400/413/422 are passed through without cooling the channel down.
pub(crate) fn penalizes_channel(status: StatusCode) -> bool {
    if status.is_client_error() {
        // 4xx that points at the channel rather than at the request payload.
        matches!(
            status.as_u16(),
            401 | 402 | 403 | 404 | 408 | 409 | 425 | 429
        )
    } else {
        // 5xx, unexpected redirects and informational responses are all
        // upstream-side failures.
        true
    }
}

pub(crate) fn upstream_error_info(value: Option<&Value>, body: &str) -> (String, String) {
    if let Some(value) = value {
        let (error_type, message) = protocol::error_fields(value, false);
        return (
            error::truncate_message(error_type.as_deref().unwrap_or("upstream_error"), 128),
            error::truncate_message(
                message.as_deref().unwrap_or("upstream request failed"),
                1024,
            ),
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
