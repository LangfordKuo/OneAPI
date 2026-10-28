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

pub fn upstream_error(protocol: Protocol, status: StatusCode, value: &Value) -> Response {
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

    client_error(protocol, status, message, error_type)
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
