use serde_json::{Map, Value};

pub mod anthropic;
pub mod chat;
pub mod error;
pub mod ir;
pub mod responses;
pub mod sse;
pub mod stream;

pub use error::ProtocolError;
pub use ir::*;
pub use stream::{StreamDecoder, StreamEncodeOptions, StreamEncoder};

/// Extract the `(error_type, message)` pair from an upstream error payload.
///
/// Every place that classifies an upstream error — the first stream frame,
/// later stream frames, the passthrough observer and the non-stream body — has
/// to agree, otherwise the same failure is reported with a different type or
/// message depending on when it arrived.
///
/// `code_first` selects whether `code` or `type` wins when an object carries
/// both; Responses `response.failed` payloads prefer `code`.
pub(crate) fn error_fields(value: &Value, code_first: bool) -> (Option<String>, Option<String>) {
    let error = value.get("error").unwrap_or(value);
    let type_field = || error.get("type").and_then(Value::as_str);
    let code_field = || value.get("code").and_then(Value::as_str);
    let error_type = if code_first {
        code_field().or_else(type_field)
    } else {
        type_field().or_else(code_field)
    };
    // `error` is sometimes a bare string rather than an object.
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .or_else(|| value.get("message").and_then(Value::as_str));

    (error_type.map(str::to_string), message.map(str::to_string))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Chat,
    Responses,
    Anthropic,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn parse_request(self, value: &Value) -> Result<UnifiedRequest, ProtocolError> {
        match self {
            Self::Chat => chat::parse_request(value),
            Self::Responses => responses::parse_request(value),
            Self::Anthropic => anthropic::parse_request(value),
        }
    }

    pub fn encode_request(self, request: &UnifiedRequest) -> Result<Value, ProtocolError> {
        match self {
            Self::Chat => chat::encode_request(request),
            Self::Responses => responses::encode_request(request),
            Self::Anthropic => anthropic::encode_request(request),
        }
    }

    pub fn parse_response(self, value: &Value) -> Result<UnifiedResponse, ProtocolError> {
        match self {
            Self::Chat => chat::parse_response(value),
            Self::Responses => responses::parse_response(value),
            Self::Anthropic => anthropic::parse_response(value),
        }
    }

    pub fn encode_response(self, response: &UnifiedResponse) -> Result<Value, ProtocolError> {
        match self {
            Self::Chat => chat::encode_response(response),
            Self::Responses => responses::encode_response(response),
            Self::Anthropic => anthropic::encode_response(response),
        }
    }
}

pub(crate) fn get_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

pub(crate) fn get_string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    get_str(obj, key).map(str::to_string)
}

pub(crate) fn get_u32(obj: &Map<String, Value>, key: &str) -> Option<u32> {
    obj.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

pub(crate) fn get_u64(obj: &Map<String, Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(Value::as_u64)
}

pub(crate) fn get_bool(obj: &Map<String, Value>, key: &str) -> Option<bool> {
    obj.get(key).and_then(Value::as_bool)
}

pub(crate) fn parse_arguments(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

// Bounds for client-supplied generation controls. Values are validated in the
// request parsers so a malformed request fails with 400 instead of being
// silently forwarded or dropped.
pub(crate) const MIN_MAX_TOKENS: u32 = 1;
pub(crate) const MAX_MAX_TOKENS: u32 = 1_000_000;
pub(crate) const MIN_N: u32 = 1;
pub(crate) const MAX_N: u32 = 128;
pub(crate) const MIN_TEMPERATURE: f64 = 0.0;
pub(crate) const MAX_TEMPERATURE: f64 = 2.0;
pub(crate) const MIN_TOP_P: f64 = 0.0;
pub(crate) const MAX_TOP_P: f64 = 1.0;

/// Parse an optional integer field, rejecting a present value that is not an
/// integer or falls outside `min..=max`.
///
/// A present-but-wrong-type field must not be treated as absent: the client
/// asked for a control the gateway cannot honour, so that is a 400.
pub(crate) fn parse_bounded_u32(
    obj: &Map<String, Value>,
    key: &str,
    protocol: &'static str,
    min: u32,
    max: u32,
) -> Result<Option<u32>, ProtocolError> {
    let Some(value) = obj.get(key) else {
        return Ok(None);
    };
    let number = value.as_u64().ok_or_else(|| {
        ProtocolError::invalid_request(protocol, format!("`{key}` must be an integer"))
    })?;
    let number = u32::try_from(number).map_err(|_| {
        ProtocolError::invalid_request(protocol, format!("`{key}` must be between {min} and {max}"))
    })?;
    if !(min..=max).contains(&number) {
        return Err(ProtocolError::invalid_request(
            protocol,
            format!("`{key}` must be between {min} and {max}"),
        ));
    }
    Ok(Some(number))
}

/// Parse an optional floating point field, rejecting a present value that is
/// not a number or falls outside `min..=max`.
pub(crate) fn parse_bounded_f64(
    obj: &Map<String, Value>,
    key: &str,
    protocol: &'static str,
    min: f64,
    max: f64,
) -> Result<Option<f64>, ProtocolError> {
    let Some(value) = obj.get(key) else {
        return Ok(None);
    };
    let number = value.as_f64().ok_or_else(|| {
        ProtocolError::invalid_request(protocol, format!("`{key}` must be a number"))
    })?;
    if !number.is_finite() || number < min || number > max {
        return Err(ProtocolError::invalid_request(
            protocol,
            format!("`{key}` must be between {min} and {max}"),
        ));
    }
    Ok(Some(number))
}

/// Validate a completion token limit (`max_tokens` / `max_output_tokens`).
pub(crate) fn parse_max_tokens(
    obj: &Map<String, Value>,
    key: &str,
    protocol: &'static str,
) -> Result<Option<u32>, ProtocolError> {
    parse_bounded_u32(obj, key, protocol, MIN_MAX_TOKENS, MAX_MAX_TOKENS)
}

pub(crate) fn parse_stop(
    value: Option<&Value>,
    protocol: &'static str,
) -> Result<Vec<String>, ProtocolError> {
    match value {
        Some(Value::String(value)) => Ok(if value.is_empty() {
            Vec::new()
        } else {
            vec![value.clone()]
        }),
        Some(Value::Array(values)) => {
            let mut result = Vec::with_capacity(values.len());
            for value in values {
                let value = value.as_str().ok_or_else(|| {
                    ProtocolError::invalid_request(protocol, "stop sequences must all be strings")
                })?;
                // Empty stop sequences would never match and only confuse the
                // upstream, so they are dropped rather than forwarded.
                if !value.is_empty() {
                    result.push(value.to_string());
                }
            }
            Ok(result)
        }
        _ => Ok(Vec::new()),
    }
}

pub(crate) fn parse_effort(
    value: &str,
    protocol: &'static str,
) -> Result<ReasoningEffort, ProtocolError> {
    match value {
        "minimal" => Ok(ReasoningEffort::Minimal),
        "low" => Ok(ReasoningEffort::Low),
        "medium" => Ok(ReasoningEffort::Medium),
        "high" => Ok(ReasoningEffort::High),
        // The IR has no `none` effort, so an explicit `none` (or a typo) cannot
        // be represented faithfully. Reject it instead of silently mapping it
        // to `medium`.
        other => Err(ProtocolError::invalid_request(
            protocol,
            format!("unsupported reasoning effort `{other}`"),
        )),
    }
}

pub(crate) fn effort_name(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    }
}

pub(crate) fn budget_for_effort(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Minimal => 1024,
        ReasoningEffort::Low => 2048,
        ReasoningEffort::Medium => 8192,
        ReasoningEffort::High => 16384,
    }
}

pub(crate) fn effort_for_budget(budget: u32) -> ReasoningEffort {
    if budget <= 1024 {
        ReasoningEffort::Minimal
    } else if budget < 8192 {
        ReasoningEffort::Low
    } else if budget < 16384 {
        ReasoningEffort::Medium
    } else {
        ReasoningEffort::High
    }
}

pub(crate) fn reject_extras(
    request: &UnifiedRequest,
    target: &'static str,
    fields: &[&str],
) -> Result<(), ProtocolError> {
    for field in fields {
        if request.extras.contains_key(*field) {
            return Err(ProtocolError::unsupported(*field, target));
        }
    }
    Ok(())
}

pub(crate) fn ensure_text_only(
    parts: &[ContentPart],
    target: &'static str,
) -> Result<(), ProtocolError> {
    if parts
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }))
    {
        return Err(ProtocolError::unsupported("image in tool_result", target));
    }
    Ok(())
}
