use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result};
use reqwest::{Client, RequestBuilder};

use crate::{models::channel::Channel, security::Crypto};

pub const DEFAULT_TIMEOUT_MS: u64 = 600_000;
const CONNECT_TIMEOUT_SECS: u64 = 10;

pub fn build_client(channel: &Channel) -> Result<Client> {
    let timeout_ms = channel
        .timeout_ms
        .map(|value| value.max(1) as u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS);

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
) -> Result<RequestBuilder> {
    let api_key = crypto
        .decrypt(&channel.api_key_enc)
        .context("failed to decrypt channel API key")?;

    let mut request = match channel.auth_header.as_str() {
        "x-api-key" => request.header("x-api-key", api_key),
        _ => request.bearer_auth(api_key),
    };

    let mut has_anthropic_version = false;
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

pub fn build_request(
    client: &Client,
    channel: &Channel,
    crypto: &Crypto,
    body: &serde_json::Value,
) -> Result<RequestBuilder> {
    let request = client.post(endpoint_url(channel)).json(body);
    apply_auth(request, channel, crypto)
}
