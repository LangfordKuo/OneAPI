use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::protocol::Protocol;

pub fn client_error_value(
    protocol: Protocol,
    message: impl Into<String>,
    error_type: &str,
) -> Value {
    let message = message.into();
    match protocol {
        Protocol::Anthropic => json!({
            "type": "error",
            "error": {"type": error_type, "message": message},
        }),
        Protocol::Chat | Protocol::Responses => json!({
            "error": {"message": message, "type": error_type},
        }),
    }
}

pub fn client_error(
    protocol: Protocol,
    status: StatusCode,
    message: impl Into<String>,
    error_type: &str,
) -> Response {
    (
        status,
        Json(client_error_value(protocol, message, error_type)),
    )
        .into_response()
}

pub fn protocol_for_path(path: &str) -> Protocol {
    if path.starts_with("/v1/messages") {
        Protocol::Anthropic
    } else if path.starts_with("/v1/responses") {
        Protocol::Responses
    } else {
        Protocol::Chat
    }
}

/// Clamp an upstream-provided string before storing it in the database or
/// echoing it back. Upstream error messages are unbounded and can otherwise
/// inflate `usage_logs` and the channel failure state.
pub fn truncate_message(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

/// Replace URL tokens in upstream-provided text with `[url]`.
///
/// Error messages from providers and HTTP clients often embed the request URL,
/// which reveals internal hosts, ports and routing paths. Those parts are never
/// useful to an API key client, so they are redacted before the text is echoed
/// back.
///
/// A URL is not necessarily a whole whitespace-delimited token: it commonly
/// appears as `(https://...)`, `url=https://...` or `<https://...>`. The scan
/// therefore looks inside each token and replaces everything from the scheme
/// onward, keeping the prefix and all surrounding whitespace intact.
pub(crate) fn redact_urls(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut token_start: Option<usize> = None;

    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(start) = token_start.take() {
                redact_token(&mut output, &text[start..index]);
            }
            output.push(character);
        } else if token_start.is_none() {
            token_start = Some(index);
        }
    }
    if let Some(start) = token_start {
        redact_token(&mut output, &text[start..]);
    }

    output
}

fn redact_token(output: &mut String, token: &str) {
    // ASCII lowercasing does not change byte offsets, so the match index is
    // valid for slicing the original token.
    let lower = token.to_ascii_lowercase();
    let scheme = [lower.find("http://"), lower.find("https://")]
        .into_iter()
        .flatten()
        .min();
    match scheme {
        Some(index) => {
            output.push_str(&token[..index]);
            output.push_str("[url]");
        }
        None => output.push_str(token),
    }
}

/// Wording that is safe to return to an API key client for an upstream failure.
///
/// Connection-level failures and channel configuration errors carry internal
/// hosts and ports in their raw text, so the client only receives a fixed
/// classification. A structured provider error (a JSON error body) is useful to
/// the caller and is kept, but only after URL redaction and clamping.
pub(crate) fn client_safe_upstream_message(
    error_type: &str,
    raw: &str,
    provider_json: bool,
) -> String {
    match error_type {
        "upstream_unreachable" => "upstream request failed".to_string(),
        "upstream_configuration_error" => "upstream channel is misconfigured".to_string(),
        "upstream_stream_timeout" => "upstream stream timed out".to_string(),
        "upstream_invalid_content_type"
        | "upstream_invalid_frame"
        | "upstream_invalid_response" => "upstream returned an invalid response".to_string(),
        _ if !provider_json => "upstream returned an invalid response".to_string(),
        _ => truncate_message(&redact_urls(raw), 300),
    }
}
