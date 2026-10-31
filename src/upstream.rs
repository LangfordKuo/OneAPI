use std::{collections::HashMap, sync::RwLock, time::Duration};

use anyhow::{anyhow, Context, Result};
use axum::http::{HeaderMap, HeaderValue};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder};

use crate::{
    models::channel::{Channel, ProtocolConfig},
    security::Crypto,
};

const CONNECT_TIMEOUT_SECS: u64 = 10;

/// Upper bound for a non-streaming upstream response body. Without a limit a
/// misbehaving or compromised upstream can exhaust process memory before the
/// gateway ever parses the JSON.
pub const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Read an upstream response body as bytes, refusing to buffer more than
/// [`limit`] bytes. `Content-Length` is checked first and chunked responses are
/// counted as they arrive.
pub async fn read_body_limited_bytes(response: reqwest::Response) -> Result<bytes::Bytes> {
    read_body_limited_bytes_to(response, MAX_RESPONSE_BODY_BYTES).await
}

pub async fn read_body_limited_bytes_to(
    response: reqwest::Response,
    limit: usize,
) -> Result<bytes::Bytes> {
    if let Some(length) = response.content_length() {
        if length > limit as u64 {
            return Err(anyhow!(
                "upstream response is too large ({length} bytes; limit {limit})"
            ));
        }
    }

    let capacity = response
        .content_length()
        .map(|length| length.min(limit as u64) as usize)
        .unwrap_or(0);
    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::with_capacity(capacity);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("failed to read the upstream response body")?;
        if buffer.len().saturating_add(chunk.len()) > limit {
            return Err(anyhow!("upstream response exceeded the {limit} byte limit"));
        }
        buffer.extend_from_slice(&chunk);
    }

    Ok(bytes::Bytes::from(buffer))
}

/// Read an upstream response body, refusing to buffer more than [`limit`]
/// bytes. `Content-Length` is checked first and chunked responses are counted
/// as they arrive.
pub async fn read_body_limited(response: reqwest::Response) -> Result<String> {
    read_body_limited_to(response, MAX_RESPONSE_BODY_BYTES).await
}

pub async fn read_body_limited_to(response: reqwest::Response, limit: usize) -> Result<String> {
    let bytes = read_body_limited_bytes_to(response, limit).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Client headers that are safe to forward to upstream providers. Authentication
/// and hop-by-hop headers are deliberately excluded.
const FORWARDED_HEADERS: &[&str] = &[
    "anthropic-beta",
    "anthropic-version",
    "openai-beta",
    "openai-organization",
    "openai-project",
];

/// Placeholder accepted in a channel's custom header values. It is replaced
/// with [`resolve_session`]'s stable per-conversation identifier, which is what
/// providers such as opencode expect in their session header.
pub const SESSION_TEMPLATE: &str = "{{session}}";

/// Fallback used by probes and recovery checks, which have no client session.
pub const DEFAULT_SESSION_ID: &str = "oneapi";

const MAX_SESSION_ID_LEN: usize = 256;

/// Resolve the stable session identifier used for upstream header templates.
///
/// Clients may pin a conversation with `x-oneapi-session` (or opencode's own
/// `x-opencode-session`). Without one, an API key gets a stable per-key value so
/// repeated requests still share a session instead of looking like a new
/// conversation on every call.
pub fn resolve_session(headers: &HeaderMap, api_key_id: Option<i64>) -> String {
    for name in ["x-oneapi-session", "x-opencode-session"] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            let value = value.trim();
            if !value.is_empty()
                && value.len() <= MAX_SESSION_ID_LEN
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
                })
            {
                return value.to_string();
            }
        }
    }
    match api_key_id {
        Some(id) => format!("oneapi-key-{id}"),
        None => "oneapi-playground".to_string(),
    }
}

fn resolve_extra_header_value(value: &str, session_id: Option<&str>) -> String {
    if value.contains(SESSION_TEMPLATE) {
        value.replace(SESSION_TEMPLATE, session_id.unwrap_or(DEFAULT_SESSION_ID))
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ClientKey {
    channel_id: i64,
    timeout_ms: u64,
    stream: bool,
}

/// Reuses reqwest clients per channel so connection pools and TLS sessions
/// survive across requests. The key deliberately excludes `updated_at`:
/// failure accounting bumps that column on every failed attempt, and including
/// it would allocate a new client (and connection pool) per failure without
/// ever evicting the old ones. Configuration edits invalidate via `clear()`.
#[derive(Debug, Default)]
pub struct ClientCache {
    clients: RwLock<HashMap<ClientKey, Client>>,
}

impl ClientCache {
    pub fn get(&self, channel: &Channel, default_timeout_ms: u64, stream: bool) -> Result<Client> {
        let timeout_ms = channel
            .timeout_ms
            .map(|value| value.max(1) as u64)
            .unwrap_or(default_timeout_ms);
        let key = ClientKey {
            channel_id: channel.id,
            timeout_ms,
            stream,
        };

        if let Ok(clients) = self.clients.read() {
            if let Some(client) = clients.get(&key) {
                return Ok(client.clone());
            }
        }

        let client = if stream {
            build_stream_client(channel)?
        } else {
            build_client(channel, default_timeout_ms)?
        };
        if let Ok(mut clients) = self.clients.write() {
            clients.insert(key, client.clone());
        }
        Ok(client)
    }

    pub fn clear(&self) {
        if let Ok(mut clients) = self.clients.write() {
            clients.clear();
        }
    }
}

/// Follow at most three redirects, only within the original host, and never
/// downgrade HTTPS to plain HTTP. This keeps a semi-trusted upstream from
/// redirecting gateway traffic to an internal address.
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let Some(original) = attempt.previous().first() else {
            return attempt.stop();
        };
        let next = attempt.url();
        let same_host = original.host_str().is_some_and(|host| {
            next.host_str()
                .is_some_and(|next_host| host.eq_ignore_ascii_case(next_host))
        });
        let downgrade = original.scheme() == "https" && next.scheme() != "https";
        if attempt.previous().len() < 3 && same_host && !downgrade {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

pub fn build_client(channel: &Channel, default_timeout_ms: u64) -> Result<Client> {
    let timeout_ms = channel
        .timeout_ms
        .map(|value| value.max(1) as u64)
        .unwrap_or(default_timeout_ms);
    build_client_with_timeout(channel, timeout_ms)
}

/// Build a client with an explicit total timeout. Recovery probes use a short
/// cap so one unhealthy channel cannot hold the background loop for long.
pub fn build_client_with_timeout(channel: &Channel, timeout_ms: u64) -> Result<Client> {
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_millis(timeout_ms.max(1)))
        .redirect(redirect_policy())
        .user_agent(format!("OneAPI/{}", env!("CARGO_PKG_VERSION")));

    if let Some(proxy) = &channel.proxy_url {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy)
                .with_context(|| format!("invalid proxy URL for channel {}", channel.name))?,
        );
    }

    builder
        .build()
        .context("failed to build upstream HTTP client")
}

/// Streaming requests must not carry reqwest's total request timeout, otherwise
/// a long-lived SSE response is cut off. The idle timeout is enforced per chunk
/// by the gateway instead.
pub fn build_stream_client(channel: &Channel) -> Result<Client> {
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .redirect(redirect_policy())
        .user_agent(format!("OneAPI/{}", env!("CARGO_PKG_VERSION")));

    if let Some(proxy) = &channel.proxy_url {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy)
                .with_context(|| format!("invalid proxy URL for channel {}", channel.name))?,
        );
    }

    builder
        .build()
        .context("failed to build upstream streaming HTTP client")
}

pub fn stream_idle_timeout(channel: &Channel, default_idle_timeout_ms: u64) -> Duration {
    Duration::from_millis(
        channel
            .timeout_ms
            .map(|value| value.max(1) as u64)
            .unwrap_or(default_idle_timeout_ms),
    )
}

pub fn endpoint_path(kind: &str) -> &'static str {
    match kind {
        "openai_responses" => "responses",
        "anthropic" => "messages",
        _ => "chat/completions",
    }
}

pub fn endpoint_url(config: &ProtocolConfig, kind: &str) -> String {
    format!(
        "{}/{}",
        config.base_url.trim_end_matches('/'),
        endpoint_path(kind)
    )
}

pub fn models_url(config: &ProtocolConfig) -> String {
    format!("{}/models", config.base_url.trim_end_matches('/'))
}

pub fn apply_auth(
    request: RequestBuilder,
    config: &ProtocolConfig,
    crypto: &Crypto,
    kind: &str,
    client_headers: &HeaderMap,
    session_id: Option<&str>,
) -> Result<RequestBuilder> {
    let api_key = crypto
        .decrypt(&config.api_key_enc)
        .context("failed to decrypt channel API key")?;

    // `api_key` is a Zeroizing<String>: the decrypted plaintext is wiped when
    // this scope ends, while the header value has its own copy inside reqwest.
    let mut request = match config.auth_header.as_str() {
        "x-api-key" => request.header("x-api-key", api_key.as_str()),
        _ => request.bearer_auth(api_key.as_str()),
    };

    let mut has_anthropic_version = false;
    for (name, value) in forwarded_headers(client_headers) {
        if name.eq_ignore_ascii_case("anthropic-version") {
            has_anthropic_version = true;
        }
        request = request.header(name, value);
    }

    // Explicit channel headers win over client-supplied ones.
    for (name, value) in &config.extra_headers {
        if name.eq_ignore_ascii_case("anthropic-version") {
            has_anthropic_version = true;
        }
        let resolved = resolve_extra_header_value(value, session_id);
        let resolved = HeaderValue::from_str(&resolved)
            .with_context(|| format!("invalid extra header value for {name}"))?;
        request = request.header(name.as_str(), resolved);
    }

    if kind == "anthropic" && !has_anthropic_version {
        request = request.header("anthropic-version", "2023-06-01");
    }

    Ok(request)
}

fn forwarded_headers(headers: &HeaderMap) -> Vec<(&'static str, Vec<u8>)> {
    let mut result = Vec::new();
    for name in FORWARDED_HEADERS {
        if let Some(value) = headers.get(*name) {
            result.push((*name, value.as_bytes().to_vec()));
        }
    }
    result
}

pub fn build_request(
    client: &Client,
    config: &ProtocolConfig,
    crypto: &Crypto,
    kind: &str,
    body: &serde_json::Value,
    client_headers: &HeaderMap,
    session_id: Option<&str>,
) -> Result<RequestBuilder> {
    let request = client.post(endpoint_url(config, kind)).json(body);
    apply_auth(request, config, crypto, kind, client_headers, session_id)
}

/// First concrete model configured for a route, translated through the
/// channel's model mapping. `None` means the route only accepts any model
/// (`*`) or has no models, so callers must use a model-independent probe.
pub fn probe_model(config: &ProtocolConfig) -> Option<String> {
    let model = config
        .models
        .iter()
        .find(|model| !model.is_empty() && model.as_str() != "*")?;
    Some(
        config
            .model_mapping
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.clone()),
    )
}

/// Minimal non-streaming request used by connectivity tests and the automatic
/// recovery probe. It deliberately asks for very few output tokens.
pub fn probe_body(kind: &str, model: &str) -> serde_json::Value {
    match kind {
        "openai_responses" => serde_json::json!({
            "model": model,
            "input": "ping",
            "max_output_tokens": 16,
            "store": false,
        }),
        "anthropic" => serde_json::json!({
            "model": model,
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}],
        }),
        _ => serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
            "stream": false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_header_overrides_the_key_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-oneapi-session",
            HeaderValue::from_static("conversation-42"),
        );
        assert_eq!(resolve_session(&headers, Some(7)), "conversation-42");
    }

    #[test]
    fn opencode_session_header_is_accepted_and_unsafe_values_are_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-opencode-session",
            HeaderValue::from_static("sess_abc.123"),
        );
        assert_eq!(resolve_session(&headers, None), "sess_abc.123");

        headers.insert(
            "x-opencode-session",
            HeaderValue::from_static("bad value with spaces"),
        );
        assert_eq!(resolve_session(&headers, Some(3)), "oneapi-key-3");
    }

    #[test]
    fn missing_session_falls_back_to_key_or_playground() {
        assert_eq!(
            resolve_session(&HeaderMap::new(), Some(11)),
            "oneapi-key-11"
        );
        assert_eq!(
            resolve_session(&HeaderMap::new(), None),
            "oneapi-playground"
        );
    }

    #[test]
    fn session_template_is_replaced_only_when_requested() {
        assert_eq!(
            resolve_extra_header_value(SESSION_TEMPLATE, Some("abc")),
            "abc"
        );
        assert_eq!(
            resolve_extra_header_value("prefix-{{session}}-suffix", Some("abc")),
            "prefix-abc-suffix"
        );
        assert_eq!(
            resolve_extra_header_value(SESSION_TEMPLATE, None),
            DEFAULT_SESSION_ID
        );
        assert_eq!(
            resolve_extra_header_value("literal", Some("abc")),
            "literal"
        );
    }
}
