use std::collections::VecDeque;
use std::pin::Pin;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::Response,
};
use bytes::Bytes;
use futures_util::{stream::unfold, Stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::{
    models,
    protocol::{
        sse::{SseFrame, SseParser},
        Protocol, StreamDecoder, StreamEncodeOptions, StreamEncoder, StreamEvent, Usage,
    },
    state::{AppState, ProbeLease},
};

use super::{
    responses_state,
    usage::{self, UsageRecord},
};

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

#[derive(Debug)]
pub struct StreamFailure {
    pub error_type: String,
    pub message: String,
    pub retryable: bool,
}

impl StreamFailure {
    fn retryable(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            message: message.into(),
            retryable: true,
        }
    }
}

pub struct StreamJob {
    pub state: AppState,
    pub request_id: String,
    pub api_key_id: Option<i64>,
    pub source: &'static str,
    pub client_model: String,
    pub inbound: Protocol,
    pub upstream: Protocol,
    pub channel_id: i64,
    pub upstream_model: String,
    pub started: Instant,
    pub attempts: u32,
    pub idle_timeout: Duration,
    pub passthrough: bool,
    pub client_wants_usage: bool,
    pub response_id: Option<String>,
    pub store_response: bool,
    pub store_request_detail: bool,
    pub detail_request: Option<String>,
    pub stored_request: Option<Value>,
    pub stream: ByteStream,
    /// Half-open trial lease for a recovering channel; held until the stream ends.
    pub _probe: Option<ProbeLease>,
}

/// Commit the stream to the client only after the first upstream SSE frame has
/// been inspected, so an immediate error event can still trigger failover.
pub async fn start(job: StreamJob) -> Result<Response, StreamFailure> {
    let (tx, rx) = mpsc::channel::<Bytes>(32);
    let (ready_tx, ready_rx) = oneshot::channel();

    tokio::spawn(run(job, tx, ready_tx));

    match ready_rx.await {
        Ok(Ok(())) => Ok(sse_response(rx)),
        Ok(Err(failure)) => Err(failure),
        Err(_) => Err(StreamFailure::retryable(
            "upstream_stream_closed",
            "stream task exited before starting",
        )),
    }
}

fn sse_response(rx: mpsc::Receiver<Bytes>) -> Response {
    let body_stream = unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|bytes| (Ok::<Bytes, std::io::Error>(bytes), rx))
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body_stream))
        .expect("streaming response is well formed")
}

enum FrameOutcome {
    Ok,
    Disconnected,
    Failed(String, String),
}

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_FRAME: &[u8] = b": keep-alive\n\n";

enum WaitOutcome {
    Next(Option<Result<Bytes, std::io::Error>>),
    TimedOut,
    Disconnected,
}

/// Wait for the next upstream chunk while emitting SSE heartbeat comments. The
/// idle deadline is absolute, so heartbeats do not extend the upstream timeout.
async fn wait_for_upstream(
    upstream: &mut ByteStream,
    tx: &mpsc::Sender<Bytes>,
    idle_timeout: Duration,
) -> WaitOutcome {
    let deadline = Instant::now() + idle_timeout;

    loop {
        let now = Instant::now();
        if now >= deadline {
            return WaitOutcome::TimedOut;
        }
        let wait = (deadline - now).min(HEARTBEAT_INTERVAL);

        tokio::select! {
            _ = tx.closed() => return WaitOutcome::Disconnected,
            _ = tokio::time::sleep(wait) => {
                if Instant::now() >= deadline {
                    return WaitOutcome::TimedOut;
                }
                if tx.send(Bytes::from_static(HEARTBEAT_FRAME)).await.is_err() {
                    return WaitOutcome::Disconnected;
                }
            }
            result = upstream.next() => return WaitOutcome::Next(result),
        }
    }
}

async fn run(
    job: StreamJob,
    tx: mpsc::Sender<Bytes>,
    ready: oneshot::Sender<Result<(), StreamFailure>>,
) {
    let mut parser = SseParser::new();
    let mut upstream = job.stream;
    let mut pending: VecDeque<SseFrame> = VecDeque::new();
    let mut stream_done = false;

    let first = loop {
        if let Some(frame) = pending.pop_front() {
            break Some(frame);
        }
        if stream_done {
            break None;
        }
        match timeout(job.idle_timeout, upstream.next()).await {
            Ok(Some(Ok(chunk))) => pending.extend(parser.push(&chunk)),
            Ok(Some(Err(error))) => {
                let _ = ready.send(Err(StreamFailure::retryable(
                    "upstream_unreachable",
                    error.to_string(),
                )));
                return;
            }
            Ok(None) => {
                pending.extend(parser.finish());
                stream_done = true;
            }
            Err(_) => {
                let _ = ready.send(Err(StreamFailure::retryable(
                    "upstream_stream_timeout",
                    "upstream did not send a stream event before the idle timeout",
                )));
                return;
            }
        }
    };

    let Some(first) = first else {
        let _ = ready.send(Err(StreamFailure::retryable(
            "upstream_stream_empty",
            "upstream returned an empty event stream",
        )));
        return;
    };

    if let Some((error_type, message)) = frame_error(job.upstream, &first) {
        let _ = ready.send(Err(StreamFailure::retryable(error_type, message)));
        return;
    }

    if ready.send(Ok(())).is_err() {
        return;
    }

    job.state.mark_channel_success(job.channel_id).await;

    pending.push_front(first);

    let mut decoder = StreamDecoder::new(job.upstream);
    let mut encoder = (!job.passthrough).then(|| {
        StreamEncoder::new(
            job.inbound,
            StreamEncodeOptions {
                response_id: job.response_id.clone(),
                include_usage: job.client_wants_usage,
                store: job.store_response,
            },
        )
    });

    let mut tokens = Usage::default();
    let mut pending_chars: u64 = 0;
    let mut estimated_tokens: u64 = 0;
    let mut response_capture: Vec<u8> = Vec::new();
    let mut first_token_at: Option<Instant> = None;
    let mut client_disconnected = false;
    let mut stream_error: Option<(String, String)> = None;
    let mut synthesize_error = false;

    loop {
        let frame = if let Some(frame) = pending.pop_front() {
            frame
        } else if stream_done {
            break;
        } else {
            match wait_for_upstream(&mut upstream, &tx, job.idle_timeout).await {
                WaitOutcome::Disconnected => {
                    client_disconnected = true;
                    break;
                }
                WaitOutcome::TimedOut => {
                    stream_error = Some((
                        "upstream_stream_timeout".to_string(),
                        format!("upstream stream was idle for {:?}", job.idle_timeout),
                    ));
                    synthesize_error = true;
                    break;
                }
                WaitOutcome::Next(Some(Ok(chunk))) => {
                    pending.extend(parser.push(&chunk));
                    continue;
                }
                WaitOutcome::Next(Some(Err(error))) => {
                    stream_error = Some(("upstream_unreachable".to_string(), error.to_string()));
                    synthesize_error = true;
                    break;
                }
                WaitOutcome::Next(None) => {
                    pending.extend(parser.finish());
                    stream_done = true;
                    continue;
                }
            }
        };

        match process_frame(
            job.inbound,
            job.upstream,
            job.passthrough,
            job.client_wants_usage,
            job.response_id.as_deref(),
            job.store_response,
            &job.state.live,
            job.channel_id,
            &mut pending_chars,
            &mut estimated_tokens,
            &frame,
            &mut decoder,
            &mut encoder,
            &tx,
            &mut response_capture,
            &mut tokens,
            &mut first_token_at,
        )
        .await
        {
            FrameOutcome::Ok => {}
            FrameOutcome::Disconnected => {
                client_disconnected = true;
                break;
            }
            FrameOutcome::Failed(error_type, message) => {
                stream_error = Some((error_type, message));
                break;
            }
        }
    }

    if !client_disconnected {
        if synthesize_error {
            if let Some((error_type, message)) = stream_error.clone() {
                let frame = error_frame(job.inbound, &error_type, &message);
                let _ =
                    send_bytes(&tx, Bytes::from(frame.into_bytes()), &mut response_capture).await;
            }
        } else if stream_error.is_none() {
            let finish = decoder.finish();
            if let Some(encoder) = encoder.as_mut() {
                for event in finish {
                    for text in encoder.encode(&event) {
                        if send_bytes(&tx, Bytes::from(text.into_bytes()), &mut response_capture)
                            .await
                            .is_err()
                        {
                            client_disconnected = true;
                            break;
                        }
                    }
                    if client_disconnected {
                        break;
                    }
                }
            } else {
                for event in finish {
                    if let StreamEvent::Usage(usage) = event {
                        tokens.merge(&usage);
                    }
                }
            }
        }
    }

    if job.inbound == Protocol::Responses
        && job.store_response
        && job.api_key_id.is_some()
        && stream_error.is_none()
    {
        let final_response = if job.passthrough {
            decoder.final_response()
        } else {
            encoder.as_ref().and_then(StreamEncoder::final_response)
        };

        if let Some(mut value) = final_response {
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "id".into(),
                    json!(job.response_id.clone().unwrap_or_default()),
                );
                object.insert("object".into(), json!("response"));
                object.insert("store".into(), json!(true));
                if !object.contains_key("created_at") {
                    object.insert("created_at".into(), json!(chrono::Utc::now().timestamp()));
                }
            }

            if let (Some(api_key_id), Some(request)) = (job.api_key_id, &job.stored_request) {
                if let Err(error) = responses_state::store(
                    &job.state,
                    api_key_id,
                    &job.client_model,
                    request,
                    &value,
                )
                .await
                {
                    tracing::warn!(
                        error = %error,
                        request_id = %job.request_id,
                        "failed to persist Responses state"
                    );
                }
            }
        }
    }

    if job.store_request_detail {
        if let Some(request) = &job.detail_request {
            let response = String::from_utf8_lossy(&response_capture);
            if let Err(error) = models::request_detail::insert(
                &job.state.db,
                &job.request_id,
                request,
                &response,
                models::now_ms(),
            )
            .await
            {
                tracing::warn!(
                    error = %error,
                    request_id = %job.request_id,
                    "failed to store streaming request detail"
                );
            }
        }
    }

    if let Some(actual) = tokens.output_tokens {
        let extra = actual.saturating_sub(estimated_tokens);
        if extra > 0 {
            job.state.live.add_tokens(Some(job.channel_id), extra);
        }
    }

    let status_code = if client_disconnected {
        499
    } else if stream_error.is_some() {
        502
    } else {
        200
    };
    let (error_type, error_message) = if client_disconnected {
        (
            Some("client_disconnected"),
            Some("client disconnected before the stream completed"),
        )
    } else if let Some((error_type, message)) = &stream_error {
        (Some(error_type.as_str()), Some(message.as_str()))
    } else {
        (None, None)
    };

    if let Some((error_type, message)) = &stream_error {
        if !client_disconnected {
            job.state
                .mark_channel_failure(job.channel_id, &format!("{error_type}: {message}"))
                .await;
        }
    }

    let record = UsageRecord {
        request_id: &job.request_id,
        source: job.source,
        api_key_id: job.api_key_id,
        channel_id: Some(job.channel_id),
        model: &job.client_model,
        upstream_model: Some(&job.upstream_model),
        inbound_protocol: job.inbound.as_str(),
        upstream_protocol: job.upstream.as_str(),
        stream: true,
        status_code,
        attempts: job.attempts,
        error_type,
        error_message,
        usage: &tokens,
        latency_ms: job.started.elapsed().as_millis() as i64,
        first_token_ms: first_token_at.map(|at| at.duration_since(job.started).as_millis() as i64),
        created_at: models::now_ms(),
    };

    usage::record(&job.state, &record).await;

    // Keep the response body open until persistence is done so a client that has
    // consumed the stream can immediately fetch a stored Responses object.
    drop(tx);
}

#[allow(clippy::too_many_arguments)]
async fn process_frame(
    inbound: Protocol,
    upstream: Protocol,
    passthrough: bool,
    client_wants_usage: bool,
    response_id: Option<&str>,
    store_response: bool,
    live: &crate::metrics::LiveMetrics,
    channel_id: i64,
    pending_chars: &mut u64,
    estimated_tokens: &mut u64,
    frame: &SseFrame,
    decoder: &mut StreamDecoder,
    encoder: &mut Option<StreamEncoder>,
    tx: &mpsc::Sender<Bytes>,
    capture: &mut Vec<u8>,
    tokens: &mut Usage,
    first_token_at: &mut Option<Instant>,
) -> FrameOutcome {
    if passthrough {
        let mut observed_error = None;
        if let Ok(events) = decoder.decode(frame) {
            for event in events {
                account_stream_tokens(live, channel_id, &event, pending_chars, estimated_tokens);
                match event {
                    StreamEvent::Usage(usage) => tokens.merge(&usage),
                    StreamEvent::Error {
                        error_type,
                        message,
                    } => observed_error = Some((error_type, message)),
                    _ => {}
                }
            }
        }

        if inbound == Protocol::Chat && !client_wants_usage && is_chat_usage_only(frame) {
            return FrameOutcome::Ok;
        }

        let bytes = if inbound == Protocol::Responses {
            rewrite_responses_frame(frame, response_id, Some(store_response))
        } else {
            Bytes::from(frame.raw.clone())
        };

        if !bytes.is_empty() {
            if send_bytes(tx, bytes, capture).await.is_err() {
                return FrameOutcome::Disconnected;
            }
            if first_token_at.is_none() && frame_has_content(upstream, frame) {
                *first_token_at = Some(Instant::now());
            }
        }

        if let Some((error_type, message)) = observed_error {
            return FrameOutcome::Failed(error_type, message);
        }

        return FrameOutcome::Ok;
    }

    let Some(encoder) = encoder.as_mut() else {
        return FrameOutcome::Ok;
    };

    let events = match decoder.decode(frame) {
        Ok(events) => events,
        Err(error) => {
            return FrameOutcome::Failed("conversion_error".to_string(), error.to_string())
        }
    };

    for event in events {
        account_stream_tokens(live, channel_id, &event, pending_chars, estimated_tokens);
        if let StreamEvent::Usage(usage) = &event {
            tokens.merge(usage);
        }
        let content = matches!(
            event,
            StreamEvent::TextDelta { .. }
                | StreamEvent::ReasoningDelta { .. }
                | StreamEvent::ToolInputDelta { .. }
        );
        let error = match &event {
            StreamEvent::Error {
                error_type,
                message,
            } => Some((error_type.clone(), message.clone())),
            _ => None,
        };

        for text in encoder.encode(&event) {
            if send_bytes(tx, Bytes::from(text.into_bytes()), capture)
                .await
                .is_err()
            {
                return FrameOutcome::Disconnected;
            }
            if content && first_token_at.is_none() {
                *first_token_at = Some(Instant::now());
            }
        }

        if let Some((error_type, message)) = error {
            return FrameOutcome::Failed(error_type, message);
        }
    }

    FrameOutcome::Ok
}

fn account_stream_tokens(
    live: &crate::metrics::LiveMetrics,
    channel_id: i64,
    event: &StreamEvent,
    pending_chars: &mut u64,
    estimated_tokens: &mut u64,
) {
    let chars = match event {
        StreamEvent::TextDelta { text, .. } => text.chars().count() as u64,
        StreamEvent::ReasoningDelta { text, .. } => text.chars().count() as u64,
        StreamEvent::ToolInputDelta { partial_json, .. } => partial_json.chars().count() as u64,
        _ => 0,
    };
    if chars == 0 {
        return;
    }

    *pending_chars += chars;
    let tokens = *pending_chars / 4;
    if tokens > 0 {
        *pending_chars -= tokens * 4;
        *estimated_tokens += tokens;
        live.add_tokens(Some(channel_id), tokens);
    }
}

async fn send_bytes(
    tx: &mpsc::Sender<Bytes>,
    bytes: Bytes,
    capture: &mut Vec<u8>,
) -> Result<(), ()> {
    let remaining = models::request_detail::MAX_DETAIL_BYTES.saturating_sub(capture.len());
    if remaining > 0 {
        capture.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
    }

    tokio::select! {
        _ = tx.closed() => Err(()),
        result = tx.send(bytes) => result.map_err(|_| ()),
    }
}

fn frame_error(protocol: Protocol, frame: &SseFrame) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(&frame.data).ok()?;
    let name = frame
        .event
        .as_deref()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");

    match protocol {
        Protocol::Chat => {
            let error = value.get("error")?;
            Some((
                error
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream_error")
                    .to_string(),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream stream error")
                    .to_string(),
            ))
        }
        Protocol::Anthropic => {
            if name != "error" {
                return None;
            }
            let error = value.get("error").unwrap_or(&value);
            Some((
                error
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream_error")
                    .to_string(),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream stream error")
                    .to_string(),
            ))
        }
        Protocol::Responses => {
            if name == "error" {
                return Some((
                    value
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("upstream_error")
                        .to_string(),
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("upstream stream error")
                        .to_string(),
                ));
            }
            if name == "response.failed" {
                let error = value
                    .get("response")
                    .and_then(|response| response.get("error"))?;
                return Some((
                    error
                        .get("code")
                        .and_then(Value::as_str)
                        .or_else(|| error.get("type").and_then(Value::as_str))
                        .unwrap_or("upstream_error")
                        .to_string(),
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("upstream stream error")
                        .to_string(),
                ));
            }
            None
        }
    }
}

fn frame_has_content(protocol: Protocol, frame: &SseFrame) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(&frame.data) else {
        return false;
    };

    match protocol {
        Protocol::Chat => value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("delta"))
            .is_some_and(|delta| {
                delta
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
                    || delta
                        .get("reasoning_content")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                    || delta
                        .get("reasoning")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                    || delta
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| !calls.is_empty())
            }),
        Protocol::Anthropic => frame.event.as_deref() == Some("content_block_delta"),
        Protocol::Responses => frame
            .event
            .as_deref()
            .is_some_and(|event| event.ends_with(".delta")),
    }
}

fn is_chat_usage_only(frame: &SseFrame) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(&frame.data) else {
        return false;
    };
    value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.is_empty())
        && value.get("usage").is_some()
}

fn rewrite_responses_frame(frame: &SseFrame, id: Option<&str>, store: Option<bool>) -> Bytes {
    if id.is_none() && store.is_none() {
        return Bytes::from(frame.raw.clone());
    }

    let Ok(mut value) = serde_json::from_str::<Value>(&frame.data) else {
        return Bytes::from(frame.raw.clone());
    };

    let mut changed = false;
    if let Some(response) = value.get_mut("response").and_then(Value::as_object_mut) {
        if let Some(id) = id {
            if response.contains_key("id") {
                response.insert("id".into(), json!(id));
                changed = true;
            }
        }
        if let Some(store) = store {
            response.insert("store".into(), json!(store));
            changed = true;
        }
    }
    if value.get("object").and_then(Value::as_str) == Some("response") && value.get("id").is_some()
    {
        if let Some(object) = value.as_object_mut() {
            if let Some(id) = id {
                object.insert("id".into(), json!(id));
            }
            if let Some(store) = store {
                object.insert("store".into(), json!(store));
            }
            changed = true;
        }
    }

    if !changed {
        return Bytes::from(frame.raw.clone());
    }

    let encoded = match &frame.event {
        Some(event) => format!("event: {event}\ndata: {value}\n\n"),
        None => format!("data: {value}\n\n"),
    };
    Bytes::from(encoded.into_bytes())
}

fn error_frame(protocol: Protocol, error_type: &str, message: &str) -> String {
    match protocol {
        Protocol::Chat => format!(
            "data: {}\n\n",
            json!({"error": {"message": message, "type": error_type}})
        ),
        Protocol::Anthropic => format!(
            "event: error\ndata: {}\n\n",
            json!({
                "type": "error",
                "error": {"type": error_type, "message": message},
            })
        ),
        Protocol::Responses => format!(
            "event: error\ndata: {}\n\n",
            json!({
                "type": "error",
                "code": error_type,
                "message": message,
                "param": null,
            })
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::stream;
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::time::timeout;

    use crate::{
        config::Config,
        db,
        models::{self, channel::ChannelWrite, now_ms},
        protocol::Protocol,
        security,
        state::AppState,
    };

    use super::*;

    async fn test_state() -> (TempDir, AppState) {
        let temp = TempDir::new().expect("temp dir");
        let data_dir = temp.path().to_path_buf();
        let config = Config {
            addr: "127.0.0.1:0".parse().expect("address"),
            data_dir: data_dir.clone(),
            database_url: None,
            database_path: data_dir.join("test.db"),
            log_level: "error".to_string(),
            cookie_secure: false,
            trust_proxy: false,
        };
        let db = db::connect(&config).await.expect("database");
        let crypto = std::sync::Arc::new(
            security::Crypto::load_or_create(&config.data_dir).expect("master key"),
        );
        (
            temp,
            AppState {
                config: std::sync::Arc::new(config),
                db,
                crypto,
                live: std::sync::Arc::new(crate::metrics::LiveMetrics::default()),
                updates: std::sync::Arc::new(crate::state::UpdateState::default()),
                login_limiter: std::sync::Arc::new(crate::admin::LoginLimiter::default()),
                clients: std::sync::Arc::new(crate::upstream::ClientCache::default()),
                cache: std::sync::Arc::new(crate::state::RuntimeCache::default()),
                probes: std::sync::Arc::new(crate::state::ChannelProbes::default()),
                usage_writer: None,
                started_at: chrono::Utc::now(),
            },
        )
    }

    async fn seed_channel(state: &AppState) -> i64 {
        let write = ChannelWrite {
            name: "stream-test".to_string(),
            kind: "openai_chat".to_string(),
            base_url: "http://example.invalid/v1".to_string(),
            api_key_enc: state.crypto.encrypt("key").expect("encrypt"),
            auth_header: "bearer".to_string(),
            models: json!(["gpt"]).to_string(),
            model_mapping: "{}".to_string(),
            extra_headers: "{}".to_string(),
            proxy_url: None,
            priority: 10,
            weight: 1,
            timeout_ms: None,
            enabled: 1,
        };
        models::channel::create(&state.db, &write, now_ms())
            .await
            .expect("channel")
            .id
    }

    async fn seed_key(state: &AppState) -> i64 {
        let key = security::generate_api_key();
        let hash = security::api_key_hash(&key);
        let prefix: String = key.chars().take(12).collect();
        models::api_key::create(&state.db, "stream-test", &hash, &prefix, 1, None, now_ms())
            .await
            .expect("api key")
            .id
    }

    fn job(state: AppState, channel_id: i64, api_key_id: i64, stream: ByteStream) -> StreamJob {
        StreamJob {
            state,
            request_id: "req-test".to_string(),
            api_key_id: Some(api_key_id),
            source: "gateway",
            client_model: "gpt".to_string(),
            inbound: Protocol::Chat,
            upstream: Protocol::Chat,
            channel_id,
            upstream_model: "gpt".to_string(),
            started: Instant::now(),
            attempts: 1,
            idle_timeout: Duration::from_secs(1),
            passthrough: true,
            client_wants_usage: false,
            response_id: None,
            store_response: false,
            store_request_detail: false,
            detail_request: None,
            stored_request: None,
            stream,
            _probe: None,
        }
    }

    #[test]
    fn responses_passthrough_rewrites_id_and_store() {
        let data = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_upstream",
                "object": "response",
                "status": "completed",
                "store": false,
            },
        });
        let frame = SseFrame {
            event: Some("response.completed".to_string()),
            data: data.to_string(),
            raw: data.to_string().into_bytes(),
        };

        let bytes = rewrite_responses_frame(&frame, Some("resp_local"), Some(true));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("\"id\":\"resp_local\""));
        assert!(text.contains("\"store\":true"));
        assert!(!text.contains("resp_upstream"));
    }

    #[test]
    fn responses_delta_frames_are_not_rewritten() {
        let raw = b"event: response.output_text.delta\ndata: {\"delta\":\"hi\"}\n\n".to_vec();
        let frame = SseFrame {
            event: Some("response.output_text.delta".to_string()),
            data: "{\"delta\":\"hi\"}".to_string(),
            raw: raw.clone(),
        };
        assert_eq!(
            rewrite_responses_frame(&frame, Some("resp_local"), Some(true)).to_vec(),
            raw
        );
    }

    #[tokio::test]
    async fn client_disconnect_is_logged_as_499() {
        let (_temp, state) = test_state().await;
        let channel_id = seed_channel(&state).await;
        let api_key_id = seed_key(&state).await;

        // First chunk yields a valid event, then the upstream stays open forever.
        let first = b"data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt\",\"created\":1,\
                       \"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n";
        let stream: ByteStream = Box::pin(
            stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from_static(first)) })
                .chain(stream::pending()),
        );

        let response = start(job(state.clone(), channel_id, api_key_id, stream))
            .await
            .expect("stream starts");

        // Drop the body without reading it to simulate the client going away.
        drop(response.into_body());

        let logged = timeout(Duration::from_secs(3), async {
            loop {
                let row: Option<(i64, Option<String>)> = sqlx::query_as(
                    "SELECT status_code, error_type FROM usage_logs
                     WHERE request_id = 'req-test' ORDER BY id DESC LIMIT 1",
                )
                .fetch_optional(&state.db)
                .await
                .expect("query");

                if let Some(row) = row {
                    break row;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("usage row");

        assert_eq!(logged.0, 499);
        assert_eq!(logged.1.as_deref(), Some("client_disconnected"));
    }

    #[tokio::test]
    async fn first_error_event_fails_before_commit() {
        let (_temp, state) = test_state().await;
        let channel_id = seed_channel(&state).await;
        let api_key_id = seed_key(&state).await;

        let chunk = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\n\n";
        let stream: ByteStream = Box::pin(stream::once(async move {
            Ok::<Bytes, std::io::Error>(Bytes::from_static(chunk))
        }));

        let failure = match start(job(state, channel_id, api_key_id, stream)).await {
            Ok(_) => panic!("expected failure"),
            Err(failure) => failure,
        };

        assert!(failure.retryable);
        assert_eq!(failure.message, "busy");
    }

    #[tokio::test]
    async fn chat_usage_chunk_is_filtered_when_client_did_not_request_it() {
        let (_temp, state) = test_state().await;
        let channel_id = seed_channel(&state).await;
        let api_key_id = seed_key(&state).await;

        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(
                b"data: {\"id\":\"c\",\"model\":\"gpt\",\"created\":1,\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n",
            )),
            Ok(Bytes::from_static(
                b"data: {\"id\":\"c\",\"model\":\"gpt\",\"created\":1,\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
            )),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];
        let stream: ByteStream = Box::pin(stream::iter(chunks));

        let response = start(job(state, channel_id, api_key_id, stream))
            .await
            .expect("stream starts");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("\"content\":\"hello\""));
        assert!(!body.contains("\"usage\""));
        assert!(body.contains("[DONE]"));
    }

    #[tokio::test]
    async fn streaming_request_detail_is_captured() {
        let (_temp, state) = test_state().await;
        let channel_id = seed_channel(&state).await;
        let api_key_id = seed_key(&state).await;

        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(
                b"data: {\"id\":\"c\",\"model\":\"gpt\",\"created\":1,\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n",
            )),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];
        let stream: ByteStream = Box::pin(stream::iter(chunks));

        let mut stream_job = job(state.clone(), channel_id, api_key_id, stream);
        stream_job.store_request_detail = true;
        stream_job.detail_request = Some("{\"model\":\"gpt\"}".to_string());

        let response = start(stream_job).await.expect("stream starts");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert!(String::from_utf8_lossy(&bytes).contains("hello"));

        let (request_body, response_body): (String, String) =
            sqlx::query_as("SELECT request_body, response_body FROM request_details LIMIT 1")
                .fetch_one(&state.db)
                .await
                .expect("request detail");
        assert_eq!(request_body, "{\"model\":\"gpt\"}");
        assert!(response_body.contains("\"content\":\"hello\""));
        assert!(state.live.snapshot().global_tok_s > 0.0);
    }

    #[tokio::test]
    async fn cross_protocol_stream_is_converted() {
        let (_temp, state) = test_state().await;
        let channel_id = seed_channel(&state).await;
        let api_key_id = seed_key(&state).await;

        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ];
        let stream: ByteStream = Box::pin(stream::iter(chunks));

        let mut stream_job = job(state, channel_id, api_key_id, stream);
        stream_job.upstream = Protocol::Anthropic;
        stream_job.passthrough = false;

        let response = start(stream_job).await.expect("stream starts");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("\"content\":\"hello\""));
        assert!(body.contains("\"finish_reason\":\"stop\""));
        assert!(body.contains("data: [DONE]"));
    }
}
