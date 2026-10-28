use std::{collections::HashMap, sync::RwLock, time::Duration};

use anyhow::{anyhow, Context, Result};
use axum::http::HeaderMap;
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

    #[cfg(test)]
    fn len(&self) -> usize {
        self.clients
            .read()
            .map(|clients| clients.len())
            .unwrap_or_default()
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

    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_millis(timeout_ms))
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
) -> Result<RequestBuilder> {
    let api_key = crypto
        .decrypt(&config.api_key_enc)
        .context("failed to decrypt channel API key")?;

    let mut request = match config.auth_header.as_str() {
        "x-api-key" => request.header("x-api-key", api_key),
        _ => request.bearer_auth(api_key),
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
        request = request.header(name.as_str(), value.as_str());
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
) -> Result<RequestBuilder> {
    let request = client.post(endpoint_url(config, kind)).json(body);
    apply_auth(request, config, crypto, kind, client_headers)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use crate::models::channel::Channel;

    use super::{forwarded_headers, ClientCache};

    fn channel(updated_at: i64) -> Channel {
        Channel {
            id: 7,
            name: "test".to_string(),
            kind: "openai_chat".to_string(),
            kinds: r#"["openai_chat"]"#.to_string(),
            fallback_kind: Some("openai_chat".to_string()),
            protocol_configs: "[]".to_string(),
            base_url: "http://localhost".to_string(),
            api_key_enc: String::new(),
            auth_header: "bearer".to_string(),
            models: "[]".to_string(),
            model_mapping: "{}".to_string(),
            extra_headers: "{}".to_string(),
            proxy_url: None,
            priority: 0,
            weight: 1,
            timeout_ms: None,
            enabled: 1,
            last_error: None,
            last_error_at: None,
            disabled_until: None,
            consecutive_failures: 0,
            created_at: 0,
            updated_at,
        }
    }

    #[test]
    fn client_cache_reuses_entries_across_health_updates() {
        let cache = ClientCache::default();
        let _first = cache.get(&channel(1), 60_000, false).expect("first");
        let _second = cache.get(&channel(2), 60_000, false).expect("second");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn streaming_and_non_streaming_clients_are_cached_separately() {
        let cache = ClientCache::default();
        let _non_stream = cache.get(&channel(1), 60_000, false).expect("non-stream");
        let _stream = cache.get(&channel(1), 60_000, true).expect("stream");
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn forwards_only_allowlisted_client_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-beta", HeaderValue::from_static("prompt-caching"));
        headers.insert("openai-beta", HeaderValue::from_static("assistants=v2"));
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("x-api-key", HeaderValue::from_static("secret"));
        headers.insert("cookie", HeaderValue::from_static("session=secret"));

        let forwarded = forwarded_headers(&headers);
        let names: Vec<&str> = forwarded.iter().map(|(name, _)| *name).collect();

        assert_eq!(names, vec!["anthropic-beta", "openai-beta"]);
        assert!(!names.contains(&"authorization"));
        assert!(!names.contains(&"x-api-key"));
        assert!(!names.contains(&"cookie"));
    }
}
