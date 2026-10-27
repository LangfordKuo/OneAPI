use std::{collections::HashMap, sync::RwLock, time::Duration};

use anyhow::{Context, Result};
use axum::http::HeaderMap;
use reqwest::{Client, RequestBuilder};

use crate::{models::channel::Channel, security::Crypto};

const CONNECT_TIMEOUT_SECS: u64 = 10;

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
    updated_at: i64,
    timeout_ms: u64,
    stream: bool,
}

/// Reuses reqwest clients per channel so connection pools and TLS sessions
/// survive across requests. Keys include `updated_at`, so editing a channel
/// naturally invalidates its client.
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
            updated_at: channel.updated_at,
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

pub fn build_client(channel: &Channel, default_timeout_ms: u64) -> Result<Client> {
    let timeout_ms = channel
        .timeout_ms
        .map(|value| value.max(1) as u64)
        .unwrap_or(default_timeout_ms);

    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_millis(timeout_ms))
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

pub fn endpoint_url(channel: &Channel) -> String {
    format!(
        "{}/{}",
        channel.base_url.trim_end_matches('/'),
        endpoint_path(&channel.kind)
    )
}

pub fn models_url(channel: &Channel) -> String {
    format!("{}/models", channel.base_url.trim_end_matches('/'))
}

pub fn apply_auth(
    request: RequestBuilder,
    channel: &Channel,
    crypto: &Crypto,
    client_headers: &HeaderMap,
) -> Result<RequestBuilder> {
    let api_key = crypto
        .decrypt(&channel.api_key_enc)
        .context("failed to decrypt channel API key")?;

    let mut request = match channel.auth_header.as_str() {
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
    let headers: HashMap<String, String> =
        serde_json::from_str(&channel.extra_headers).unwrap_or_default();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("anthropic-version") {
            has_anthropic_version = true;
        }
        request = request.header(name.as_str(), value.as_str());
    }

    if channel.kind == "anthropic" && !has_anthropic_version {
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
    channel: &Channel,
    crypto: &Crypto,
    body: &serde_json::Value,
    client_headers: &HeaderMap,
) -> Result<RequestBuilder> {
    let request = client.post(endpoint_url(channel)).json(body);
    apply_auth(request, channel, crypto, client_headers)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::forwarded_headers;

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
