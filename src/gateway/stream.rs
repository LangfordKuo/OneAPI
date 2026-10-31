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
        self, anthropic, chat,
        sse::{SseFrame, SseParser},
        Protocol, StreamDecoder, StreamEncodeOptions, StreamEncoder, StreamEvent, Usage,
    },
    state::{AppState, ProbeLease},
};

use super::{
    error, responses_state,
    usage::{self, UsageRecord},
};

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

#[derive(Debug)]
pub struct StreamFailure {
    pub error_type: String,
    /// Raw failure text for the usage log and channel health state. It is never
    /// written to the client as-is.
    pub message: String,
    /// Whether `message` came from a structured provider JSON error, in which
    /// case it may be shown to the client after redaction.
    pub provider_json: bool,
}

impl StreamFailure {
    /// Every failure raised before the first frame is written is retryable: no
    /// byte has reached the client yet, so a different channel may still serve
    /// the request.
    fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            message: message.into(),
            provider_json: false,
        }
    }

    /// A failure carrying an upstream error body that was already parsed as
    /// JSON.
    fn from_provider(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            message: message.into(),
            provider_json: true,
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
    /// Absolute deadline for receiving the first frame; after this the request
    /// is no longer committed to this channel.
    pub commit_deadline: Instant,
    pub passthrough: bool,
    pub client_wants_usage: bool,
    /// Approximate input token count, used when the upstream omits usage.
    pub input_token_estimate: u64,
    pub response_id: Option<String>,
    pub store_response: bool,
    pub store_request_detail: bool,
    pub detail_request: Option<String>,
    pub stored_request: Option<Value>,
    pub stream: ByteStream,
    /// Half-open trial lease for a recovering channel; held until the stream ends.
    pub _probe: Option<ProbeLease>,
    /// Only touch `channels` after a success when this channel may actually
    /// have failure state recorded; a healthy channel would otherwise pay for a
    /// no-op UPDATE (and its SQLite write lock) on every request.
    pub clear_failure_state: bool,
}

/// Clamp upstream error text before it reaches the client, the usage log, or
/// the channel health columns.
fn bounded_error(error_type: String, message: String) -> (String, String) {
    (
        error::truncate_message(&error_type, 128),
        error::truncate_message(&message, 1024),
    )
}

/// Text that may be written to the client for an in-stream failure.
///
/// Upstream-provided messages go through the shared mapping (URL redaction and
/// clamping). `conversion_error` describes the request the client itself sent,
/// so it is kept verbatim and merely clamped.
fn client_stream_message(error_type: &str, message: &str, provider_json: bool) -> String {
    if error_type == "conversion_error" {
        return error::truncate_message(message, 300);
    }
    error::client_safe_upstream_message(error_type, message, provider_json)
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
        Err(_) => Err(StreamFailure::new(
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
    /// Responses terminal frame(s) held back until persistence decides `store`.
    Completed(Vec<Bytes>),
}

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_FRAME: &[u8] = b": keep-alive\n\n";
/// Cap the bytes buffered for a single unterminated SSE frame. Without this an
/// upstream can stream an endless line and pin memory in the parser.
const MAX_SSE_BUFFER: usize = 8 * 1024 * 1024;

enum WaitOutcome {
    Next(Option<Result<Bytes, std::io::Error>>),
    TimedOut,
    /// The client stopped reading: the outbound channel stayed full past the
    /// idle deadline, so this is not an upstream failure.
    ClientBlocked,
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
                // The heartbeat must not block past the absolute idle deadline.
                // A full channel means the client stopped reading, which is a
                // client-side problem rather than upstream idleness.
                tokio::select! {
                    _ = tx.closed() => return WaitOutcome::Disconnected,
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        return WaitOutcome::ClientBlocked;
                    }
                    result = tx.send(Bytes::from_static(HEARTBEAT_FRAME)) => {
                        if result.is_err() {
                            return WaitOutcome::Disconnected;
                        }
                    }
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
        let now = Instant::now();
        if now >= job.commit_deadline {
            let _ = ready.send(Err(StreamFailure::new(
                "upstream_stream_timeout",
                "upstream did not send the first event before the commit deadline",
            )));
            return;
        }
        // The idle timeout is per chunk, but committing to a channel must have
        // an absolute deadline: a trickle of partial data must not hold the
        // request open indefinitely.
        let wait = job.idle_timeout.min(job.commit_deadline - now);
        match timeout(wait, upstream.next()).await {
            Ok(Some(Ok(chunk))) => {
                pending.extend(parser.push(&chunk));
                if parser.buffered_len() > MAX_SSE_BUFFER {
                    let _ = ready.send(Err(StreamFailure::new(
                        "upstream_stream_too_large",
                        "upstream stream frame exceeded the size limit",
                    )));
                    return;
                }
            }
            Ok(Some(Err(error))) => {
                let _ = ready.send(Err(StreamFailure::new(
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
                let committed_late = Instant::now() >= job.commit_deadline;
                let _ = ready.send(Err(StreamFailure::new(
                    "upstream_stream_timeout",
                    if committed_late {
                        "upstream did not send the first event before the commit deadline"
                    } else {
                        "upstream did not send a stream event before the idle timeout"
                    },
                )));
                return;
            }
        }
    };

    let Some(first) = first else {
        let _ = ready.send(Err(StreamFailure::new(
            "upstream_stream_empty",
            "upstream returned an empty event stream",
        )));
        return;
    };

    if let Some((error_type, message)) = frame_error(job.upstream, &first) {
        let (error_type, message) = bounded_error(error_type, message);
        let _ = ready.send(Err(StreamFailure::from_provider(error_type, message)));
        return;
    }

    if ready.send(Ok(())).is_err() {
        return;
    }

    job.state
        .mark_channel_success_if_needed(job.channel_id, job.clear_failure_state)
        .await;

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
    // Raw stream_error text is kept for logging and channel health; this flag
    // selects the client-facing wording when the error frame is emitted.
    let mut stream_error_provider_json = false;
    let mut synthesize_error = false;
    let mut held_terminal: Option<Vec<Bytes>> = None;
    // Same-protocol passthrough does not run every frame through the decoder,
    // so its terminal frame is tracked from the observation instead.
    let mut passthrough_terminal = false;

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
                    stream_error = Some(bounded_error(
                        "upstream_stream_timeout".to_string(),
                        format!("upstream stream was idle for {:?}", job.idle_timeout),
                    ));
                    stream_error_provider_json = false;
                    synthesize_error = true;
                    break;
                }
                WaitOutcome::ClientBlocked => {
                    // The heartbeat could not be written before the idle
                    // deadline: treat it as a slow/disconnected client and
                    // leave channel health untouched.
                    tracing::warn!(
                        request_id = %job.request_id,
                        channel_id = job.channel_id,
                        "client_slow: heartbeat blocked past the idle deadline"
                    );
                    client_disconnected = true;
                    break;
                }
                WaitOutcome::Next(Some(Ok(chunk))) => {
                    pending.extend(parser.push(&chunk));
                    if parser.buffered_len() > MAX_SSE_BUFFER {
                        stream_error = Some(bounded_error(
                            "upstream_stream_too_large".to_string(),
                            "upstream stream frame exceeded the size limit".to_string(),
                        ));
                        stream_error_provider_json = false;
                        synthesize_error = true;
                        break;
                    }
                    continue;
                }
                WaitOutcome::Next(Some(Err(error))) => {
                    stream_error = Some(bounded_error(
                        "upstream_unreachable".to_string(),
                        error.to_string(),
                    ));
                    stream_error_provider_json = false;
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
            frame,
            &mut decoder,
            &mut encoder,
            &tx,
            &mut response_capture,
            &mut tokens,
            &mut first_token_at,
            &mut passthrough_terminal,
        )
        .await
        {
            FrameOutcome::Ok => {}
            FrameOutcome::Disconnected => {
                client_disconnected = true;
                break;
            }
            FrameOutcome::Failed(error_type, message) => {
                // `conversion_error` is gateway wording about the payload the
                // client sent; every other failure comes from a structured
                // upstream error event.
                stream_error_provider_json = error_type != "conversion_error";
                stream_error = Some(bounded_error(error_type, message));
                break;
            }
            FrameOutcome::Completed(held) => {
                held_terminal = Some(held);
                break;
            }
        }
    }

    // An upstream that closes without its protocol terminal frame truncated
    // the stream even when every byte it sent was valid SSE. Report it instead
    // of encoding a synthetic completion the client would trust.
    if !client_disconnected && stream_error.is_none() && held_terminal.is_none() {
        let saw_terminal = if job.passthrough {
            passthrough_terminal
        } else {
            decoder.saw_terminal()
        };
        if !saw_terminal {
            stream_error = Some(bounded_error(
                "upstream_stream_truncated".to_string(),
                "upstream stream ended without a terminal event".to_string(),
            ));
            stream_error_provider_json = false;
            synthesize_error = true;
        }
    }

    if !client_disconnected {
        if synthesize_error {
            if let Some((error_type, message)) = stream_error.clone() {
                // `stream_error` keeps the raw text for the usage row and the
                // channel health state above; only the frame is sanitized.
                let client_message =
                    client_stream_message(&error_type, &message, stream_error_provider_json);
                let frame = error_frame(job.inbound, &error_type, &client_message);
                let _ =
                    send_bytes(&tx, Bytes::from(frame.into_bytes()), &mut response_capture).await;
            }
        } else if stream_error.is_none() && held_terminal.is_none() {
            let finish = decoder.finish();
            if let Some(encoder) = encoder.as_mut() {
                for event in finish {
                    if job.inbound == Protocol::Responses && matches!(&event, StreamEvent::Done) {
                        let held: Vec<Bytes> = encoder
                            .encode(&event)
                            .into_iter()
                            .map(|text| Bytes::from(text.into_bytes()))
                            .collect();
                        held_terminal = Some(held);
                        continue;
                    }
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

    if stream_error.is_none() {
        if let Some(held) = held_terminal.take() {
            if flush_responses_completion(
                &job.state,
                &job.request_id,
                job.api_key_id,
                &job.client_model,
                job.response_id.as_deref(),
                job.stored_request.as_ref(),
                job.store_response,
                job.passthrough,
                &decoder,
                encoder.as_ref(),
                held,
                &tx,
                &mut response_capture,
            )
            .await
            {
                client_disconnected = true;
            }
        }
    }

    if job.store_request_detail {
        if let Some(request) = &job.detail_request {
            let captured = String::from_utf8_lossy(&response_capture);
            // Bound the body before the record copies it into the write queue.
            let response = models::request_detail::truncate_detail(&captured);
            usage::record_detail(
                &job.state,
                &usage::DetailRecord {
                    request_id: &job.request_id,
                    api_key_id: job.api_key_id.unwrap_or(0),
                    request_body: request,
                    response_body: &response,
                    created_at: models::now_ms(),
                },
            )
            .await;
        }
    }

    if let Some(actual) = tokens.output_tokens {
        let extra = actual.saturating_sub(estimated_tokens);
        if extra > 0 {
            job.state.live.add_tokens(Some(job.channel_id), extra);
        }
    }

    // A stream that is interrupted (or an upstream that never reports usage)
    // must still count against the API key quota. Prefer reported values and
    // fall back to the locally accumulated estimate.
    let mut recorded_usage = tokens.clone();
    recorded_usage.fill_partial_estimate(job.input_token_estimate, estimated_tokens);

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
        usage: &recorded_usage,
        latency_ms: job.started.elapsed().as_millis() as i64,
        first_token_ms: first_token_at.map(|at| at.duration_since(job.started).as_millis() as i64),
        created_at: models::now_ms(),
    };

    usage::record(&job.state, &record).await;

    // Keep the response body open until persistence is done so a client that has
    // consumed the stream can immediately fetch a stored Responses object.
    drop(tx);
}

/// Persist a held Responses terminal frame, then send it with a truthful
/// `store` value. Returns whether the client disconnected while sending.
#[allow(clippy::too_many_arguments)]
async fn flush_responses_completion(
    state: &AppState,
    request_id: &str,
    api_key_id: Option<i64>,
    client_model: &str,
    response_id: Option<&str>,
    stored_request: Option<&Value>,
    store_response: bool,
    passthrough: bool,
    decoder: &StreamDecoder,
    encoder: Option<&StreamEncoder>,
    held: Vec<Bytes>,
    tx: &mpsc::Sender<Bytes>,
    capture: &mut Vec<u8>,
) -> bool {
    let should_store = store_response && api_key_id.is_some();
    let mut stored = false;

    if should_store {
        let final_response = if passthrough {
            decoder.final_response()
        } else {
            encoder.and_then(StreamEncoder::final_response)
        };

        if let Some(mut value) = final_response {
            if let Some(object) = value.as_object_mut() {
                object.insert("id".into(), json!(response_id.unwrap_or_default()));
                object.insert("object".into(), json!("response"));
                object.insert("store".into(), json!(true));
                if !object.contains_key("created_at") {
                    object.insert("created_at".into(), json!(chrono::Utc::now().timestamp()));
                }
            }

            if let (Some(api_key_id), Some(request)) = (api_key_id, stored_request) {
                stored =
                    match responses_state::store(state, api_key_id, client_model, request, &value)
                        .await
                    {
                        Ok(stored) => stored,
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                request_id = %request_id,
                                "failed to persist Responses state"
                            );
                            false
                        }
                    };
                if !stored {
                    tracing::warn!(
                        request_id = %request_id,
                        "streamed response was too large to persist"
                    );
                }
            }
        }
    }

    for bytes in held {
        let bytes = if should_store && !stored {
            rewrite_terminal_store(&bytes)
        } else {
            bytes
        };
        if send_bytes(tx, bytes, capture).await.is_err() {
            return true;
        }
    }
    false
}

/// Re-emit a held Responses terminal frame with `store:false`. Only used when
/// persistence was skipped, so the client is never told the object exists.
fn rewrite_terminal_store(bytes: &Bytes) -> Bytes {
    let mut parser = SseParser::new();
    let mut frames = parser.push(bytes);
    frames.extend(parser.finish());
    if frames.is_empty() {
        return bytes.clone();
    }

    let mut output = Vec::with_capacity(bytes.len());
    for frame in frames {
        match rewrite_responses_frame(&frame, None, Some(false)) {
            Some(rewritten) => output.extend_from_slice(&rewritten),
            None => output.extend_from_slice(&frame.raw),
        }
    }
    Bytes::from(output)
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
    frame: SseFrame,
    decoder: &mut StreamDecoder,
    encoder: &mut Option<StreamEncoder>,
    tx: &mpsc::Sender<Bytes>,
    capture: &mut Vec<u8>,
    tokens: &mut Usage,
    first_token_at: &mut Option<Instant>,
    passthrough_terminal: &mut bool,
) -> FrameOutcome {
    if passthrough {
        let observation = observe_passthrough(upstream, &frame);
        if observation.saw_terminal {
            *passthrough_terminal = true;
        }
        account_stream_chars(
            live,
            channel_id,
            observation.chars,
            pending_chars,
            estimated_tokens,
        );
        if let Some(usage) = observation.usage {
            tokens.merge(&usage);
        }

        // A stored Responses object is rebuilt from the terminal payload, so
        // only that frame needs the stateful decoder. Everything else stays on
        // the allocation-free observation path.
        if upstream == Protocol::Responses && observation.terminal {
            if let Ok(events) = decoder.decode(&frame) {
                for event in events {
                    if let StreamEvent::Usage(usage) = event {
                        tokens.merge(&usage);
                    }
                }
            }
        }

        if inbound == Protocol::Chat && !client_wants_usage && observation.usage_only {
            return FrameOutcome::Ok;
        }

        let has_content = first_token_at.is_none() && observation.has_content;
        let bytes = if inbound == Protocol::Responses {
            let rewritten = rewrite_responses_frame(&frame, response_id, Some(store_response));
            rewritten.unwrap_or_else(|| Bytes::from(frame.raw))
        } else {
            // Same protocol: forward the original bytes without copying them.
            Bytes::from(frame.raw)
        };

        // Defer the terminal Responses frame so `store` reflects whether the
        // response was actually persisted.
        if inbound == Protocol::Responses && observation.terminal {
            return FrameOutcome::Completed(vec![bytes]);
        }

        if !bytes.is_empty() {
            if send_bytes(tx, bytes, capture).await.is_err() {
                return FrameOutcome::Disconnected;
            }
            if has_content {
                *first_token_at = Some(Instant::now());
            }
        }

        if let Some((error_type, message)) = observation.error {
            return FrameOutcome::Failed(error_type, message);
        }

        return FrameOutcome::Ok;
    }

    let Some(encoder) = encoder.as_mut() else {
        return FrameOutcome::Ok;
    };

    let events = match decoder.decode(&frame) {
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

        // Hold the terminal batch (block closes + `response.completed`) until
        // the caller has attempted persistence.
        if inbound == Protocol::Responses && matches!(&event, StreamEvent::Done) {
            let held = encoder
                .encode(&event)
                .into_iter()
                .map(|text| Bytes::from(text.into_bytes()))
                .collect();
            return FrameOutcome::Completed(held);
        }

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
    account_stream_chars(live, channel_id, chars, pending_chars, estimated_tokens);
}

fn account_stream_chars(
    live: &crate::metrics::LiveMetrics,
    channel_id: i64,
    chars: u64,
    pending_chars: &mut u64,
    estimated_tokens: &mut u64,
) {
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

#[derive(Default)]
struct PassthroughObservation {
    chars: u64,
    usage: Option<Usage>,
    error: Option<(String, String)>,
    /// Responses `response.completed` / `response.incomplete`, i.e. the frame
    /// that must be held back until persistence decides `store`.
    terminal: bool,
    /// The protocol's terminal frame was seen: Chat `[DONE]`, Anthropic
    /// `message_stop`, or a Responses terminal event. Used to detect a stream
    /// that ends without one.
    saw_terminal: bool,
    /// First frame that actually carries generated content, used to time the
    /// first token.
    has_content: bool,
    /// Chat usage-only frame: no choices, only a usage block. Dropped when the
    /// client did not ask for usage.
    usage_only: bool,
}

/// Observe a frame on the same-protocol passthrough path.
///
/// Token accounting only needs character counts, so this parses the payload
/// once and never builds IR events or copies delta strings. The stateful
/// decoder is reserved for terminal Responses frames (storage needs the final
/// object) and for cross-protocol conversion.
fn observe_passthrough(protocol: Protocol, frame: &SseFrame) -> PassthroughObservation {
    // Anthropic and Responses name their delta events, so first-token timing
    // needs no JSON. Chat decides below, from the delta body.
    let mut observation = PassthroughObservation {
        has_content: match protocol {
            Protocol::Anthropic => frame.event.as_deref() == Some("content_block_delta"),
            Protocol::Responses => frame
                .event
                .as_deref()
                .is_some_and(|event| event.ends_with(".delta")),
            Protocol::Chat => false,
        },
        ..PassthroughObservation::default()
    };

    if protocol == Protocol::Chat && frame.data.trim() == "[DONE]" {
        observation.saw_terminal = true;
        return observation;
    }

    let Ok(value) = serde_json::from_str::<Value>(&frame.data) else {
        return observation;
    };
    let name = frame
        .event
        .as_deref()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");

    match protocol {
        Protocol::Chat => {
            if value.get("error").is_some() {
                observation.error = Some(observed_error(&value));
            }
            if let Some(usage) = value.get("usage").and_then(Value::as_object) {
                observation.usage = Some(chat::parse_usage(usage));
            }

            let choices = value.get("choices").and_then(Value::as_array);
            observation.usage_only =
                choices.is_some_and(|choices| choices.is_empty()) && value.get("usage").is_some();

            if let Some(delta) = choices
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("delta"))
            {
                observation.has_content = delta_has_content(delta);
                observation.chars += char_count(delta.get("content"));
                observation.chars += char_count(
                    delta
                        .get("reasoning_content")
                        .or_else(|| delta.get("reasoning")),
                );
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        observation.chars += char_count(
                            call.get("function")
                                .and_then(|function| function.get("arguments")),
                        );
                    }
                }
            }
        }
        Protocol::Anthropic => match name {
            "message_start" => {
                if let Some(usage) = value
                    .get("message")
                    .and_then(|message| message.get("usage"))
                    .and_then(Value::as_object)
                {
                    observation.usage = Some(anthropic::parse_usage(usage));
                }
            }
            "message_delta" => {
                if let Some(usage) = value.get("usage").and_then(Value::as_object) {
                    observation.usage = Some(anthropic::parse_usage(usage));
                }
            }
            "content_block_delta" => {
                if let Some(delta) = value.get("delta") {
                    let text = match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text_delta" => delta.get("text"),
                        "thinking_delta" => delta.get("thinking"),
                        "input_json_delta" => delta.get("partial_json"),
                        _ => None,
                    };
                    observation.chars = char_count(text);
                }
            }
            "error" => observation.error = Some(observed_error(&value)),
            "message_stop" => observation.saw_terminal = true,
            _ => {}
        },
        Protocol::Responses => {
            match name {
                "response.completed" | "response.incomplete" => {
                    observation.terminal = true;
                    observation.saw_terminal = true;
                }
                "error" => observation.error = Some(observed_error(&value)),
                "response.failed" => {
                    observation.saw_terminal = true;
                    observation.error = Some(
                        value
                            .get("response")
                            .and_then(|response| response.get("error"))
                            .map(observed_error)
                            .unwrap_or_else(|| {
                                (
                                    "upstream_error".to_string(),
                                    "upstream response failed".to_string(),
                                )
                            }),
                    );
                }
                _ => {}
            }

            // Only the delta events the stateful decoder used to account for.
            if matches!(
                name,
                "response.output_text.delta"
                    | "response.function_call_arguments.delta"
                    | "response.reasoning_summary_text.delta"
                    | "response.reasoning_text.delta"
            ) {
                observation.chars = char_count(value.get("delta"));
            }
        }
    }

    observation
}

fn char_count(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_str)
        .map(|text| text.chars().count() as u64)
        .unwrap_or(0)
}

/// Same field extraction as the converted-stream decoder, so passthrough and
/// converted streams report the same error type/message.
fn observed_error(value: &Value) -> (String, String) {
    let (error_type, message) = protocol::error_fields(value, false);
    (
        error_type.unwrap_or_else(|| "upstream_error".to_string()),
        message.unwrap_or_else(|| "upstream stream error".to_string()),
    )
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
    // A non-SSE body (HTML error page, bare JSON document) reaches the parser
    // as a line without a `data:` field, so `data` stays empty while `raw`
    // carries bytes. It must never be committed to the client as a stream.
    // Comment-only heartbeats (`: ping`) and `event:`-only frames are valid SSE
    // and must keep flowing.
    let comment_only = frame
        .raw
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        == Some(&b':');
    if frame.data.trim().is_empty()
        && !frame.raw.is_empty()
        && frame.event.is_none()
        && !comment_only
    {
        return Some((
            "upstream_invalid_frame".to_string(),
            "upstream returned a non-SSE response".to_string(),
        ));
    }

    let value: Value = serde_json::from_str(&frame.data).ok()?;
    let name = frame
        .event
        .as_deref()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");

    let fields = |value: &Value, code_first: bool| {
        let (error_type, message) = protocol::error_fields(value, code_first);
        (
            error_type.unwrap_or_else(|| "upstream_error".to_string()),
            message.unwrap_or_else(|| "upstream stream error".to_string()),
        )
    };

    match protocol {
        // Chat has no dedicated error event: an `error` key is the signal.
        Protocol::Chat => {
            value.get("error")?;
            Some(fields(&value, false))
        }
        Protocol::Anthropic if name == "error" => Some(fields(&value, false)),
        Protocol::Responses if name == "error" => Some(fields(&value, false)),
        Protocol::Responses if name == "response.failed" => {
            let fields = value
                .get("response")
                .and_then(|response| response.get("error"))
                .map(|error| fields(error, true))
                .unwrap_or_else(|| {
                    (
                        "upstream_error".to_string(),
                        "upstream response failed".to_string(),
                    )
                });
            Some(fields)
        }
        Protocol::Anthropic | Protocol::Responses => None,
    }
}

/// Whether a Chat `delta` object carries generated content.
fn delta_has_content(delta: &Value) -> bool {
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
}

/// Rewrite the `id`/`store` fields of a Responses frame. Returns `None` when
/// the original bytes can be forwarded unchanged, which is the common case for
/// the high-frequency delta events.
fn rewrite_responses_frame(
    frame: &SseFrame,
    id: Option<&str>,
    store: Option<bool>,
) -> Option<Bytes> {
    if id.is_none() && store.is_none() {
        return None;
    }

    // Delta/content events never carry a top-level `response` object, so
    // parsing them would only burn CPU on every streamed token.
    if frame.event.as_deref().is_some_and(|event| {
        event.ends_with(".delta") || event.ends_with(".added") || event.ends_with(".done")
    }) {
        return None;
    }

    let Ok(mut value) = serde_json::from_str::<Value>(&frame.data) else {
        return None;
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
        return None;
    }

    // Keep `id:`, `retry:` and comment lines that the upstream sent alongside
    // the event we are rewriting; only the event/data payload changed.
    let mut encoded = preserve_sse_extra_lines(frame);
    if let Some(event) = &frame.event {
        encoded.push_str(&format!("event: {event}\n"));
    }
    encoded.push_str(&format!("data: {value}\n\n"));
    Some(Bytes::from(encoded.into_bytes()))
}

/// Collect the SSE fields that a rewrite must not drop: comments, `id:` and
/// `retry:`. Everything else (`event:`, `data:`, blank separators) is emitted
/// by the caller from the rewritten payload.
fn preserve_sse_extra_lines(frame: &SseFrame) -> String {
    let mut output = String::new();
    for line in frame.raw.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let is_comment = line.first() == Some(&b':');
        if !is_comment && !line.starts_with(b"id:") && !line.starts_with(b"retry:") {
            continue;
        }
        output.push_str(&String::from_utf8_lossy(line));
        output.push('\n');
    }
    output
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
