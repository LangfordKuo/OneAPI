use std::collections::{HashMap, HashSet};
use std::time::Instant;

use axum::{
    extract::{Path, State},
    http::{header::HeaderName, HeaderMap, HeaderValue, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    error::AppError,
    models::{
        self,
        channel::{Channel, ChannelWrite},
    },
    security::Crypto,
    state::AppState,
    upstream,
};

use super::auth::AdminSession;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    OpenaiChat,
    OpenaiResponses,
    Anthropic,
}

impl ChannelKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiChat => "openai_chat",
            Self::OpenaiResponses => "openai_responses",
            Self::Anthropic => "anthropic",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "openai_responses" => Self::OpenaiResponses,
            "anthropic" => Self::Anthropic,
            _ => Self::OpenaiChat,
        }
    }

    fn default_auth_header(self) -> &'static str {
        match self {
            Self::Anthropic => "x-api-key",
            Self::OpenaiChat | Self::OpenaiResponses => "bearer",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ChannelInput {
    pub name: String,
    #[serde(default)]
    pub kind: Option<ChannelKind>,
    #[serde(default)]
    pub kinds: Vec<ChannelKind>,
    #[serde(default)]
    pub fallback_kind: Option<ChannelKind>,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub weight: Option<i64>,
    #[serde(default)]
    pub timeout_ms: Option<i64>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ChannelOutput {
    pub id: i64,
    pub name: String,
    pub kind: ChannelKind,
    pub kinds: Vec<ChannelKind>,
    pub fallback_kind: ChannelKind,
    pub base_url: String,
    pub auth_header: String,
    pub api_key_masked: String,
    pub models: Vec<String>,
    pub model_mapping: HashMap<String, String>,
    pub extra_headers: HashMap<String, String>,
    pub proxy_url: Option<String>,
    pub priority: i64,
    pub weight: i64,
    pub timeout_ms: Option<i64>,
    pub enabled: bool,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    pub disabled_until: Option<i64>,
    pub consecutive_failures: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

fn output_kinds(channel: &Channel) -> (Vec<ChannelKind>, ChannelKind) {
    let stored: Vec<ChannelKind> = serde_json::from_str(&channel.kinds).unwrap_or_default();
    let mut kinds = Vec::new();
    for kind in stored {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        kinds.push(ChannelKind::from_db(&channel.kind));
    }
    let fallback = channel
        .fallback_kind
        .as_deref()
        .map(ChannelKind::from_db)
        .filter(|kind| kinds.contains(kind))
        .unwrap_or(kinds[0]);
    (kinds, fallback)
}

fn output(channel: Channel, crypto: &Crypto) -> ChannelOutput {
    let (kinds, fallback_kind) = output_kinds(&channel);
    ChannelOutput {
        id: channel.id,
        name: channel.name,
        kind: kinds[0],
        kinds,
        fallback_kind,
        base_url: channel.base_url,
        auth_header: channel.auth_header,
        api_key_masked: crypto.mask(&channel.api_key_enc),
        models: serde_json::from_str(&channel.models).unwrap_or_default(),
        model_mapping: serde_json::from_str(&channel.model_mapping).unwrap_or_default(),
        extra_headers: serde_json::from_str(&channel.extra_headers).unwrap_or_default(),
        proxy_url: channel.proxy_url,
        priority: channel.priority,
        weight: channel.weight,
        timeout_ms: channel.timeout_ms,
        enabled: channel.enabled != 0,
        last_error: channel.last_error,
        last_error_at: channel.last_error_at,
        disabled_until: channel.disabled_until,
        consecutive_failures: channel.consecutive_failures,
        created_at: channel.created_at,
        updated_at: channel.updated_at,
    }
}

struct ValidatedChannel {
    name: String,
    kinds: Vec<ChannelKind>,
    fallback_kind: ChannelKind,
    base_url: String,
    api_key: Option<String>,
    auth_header: String,
    models: Vec<String>,
    model_mapping: HashMap<String, String>,
    extra_headers: HashMap<String, String>,
    proxy_url: Option<String>,
    priority: i64,
    weight: i64,
    timeout_ms: Option<i64>,
    enabled: i64,
}

fn validate(input: ChannelInput, creating: bool) -> Result<ValidatedChannel, AppError> {
    let ChannelInput {
        name,
        kind,
        kinds,
        fallback_kind,
        base_url,
        api_key,
        auth_header,
        models,
        model_mapping,
        extra_headers,
        proxy_url,
        priority,
        weight,
        timeout_ms,
        enabled,
    } = input;

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("channel name is required".to_string()));
    }

    let mut unique_kinds = Vec::new();
    for kind in kinds {
        if !unique_kinds.contains(&kind) {
            unique_kinds.push(kind);
        }
    }
    if unique_kinds.is_empty() {
        if let Some(kind) = kind {
            unique_kinds.push(kind);
        }
    }
    if unique_kinds.is_empty() {
        return Err(AppError::BadRequest(
            "at least one channel protocol is required".to_string(),
        ));
    }
    let fallback_kind = fallback_kind.unwrap_or(unique_kinds[0]);
    if !unique_kinds.contains(&fallback_kind) {
        return Err(AppError::BadRequest(
            "fallback_kind must be one of the configured protocols".to_string(),
        ));
    }
    let primary_kind = unique_kinds[0];

    let base_url = base_url.trim().trim_end_matches('/').to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(AppError::BadRequest(
            "base_url must start with http:// or https://".to_string(),
        ));
    }

    let api_key = api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if creating && api_key.is_none() {
        return Err(AppError::BadRequest(
            "api_key is required when creating a channel".to_string(),
        ));
    }

    let auth_header = auth_header
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| primary_kind.default_auth_header().to_string());
    if auth_header != "bearer" && auth_header != "x-api-key" {
        return Err(AppError::BadRequest(
            "auth_header must be 'bearer' or 'x-api-key'".to_string(),
        ));
    }

    let mut unique_models = Vec::new();
    let mut seen = HashSet::new();
    for model in models {
        let model = model.trim();
        if !model.is_empty() && seen.insert(model.to_string()) {
            unique_models.push(model.to_string());
        }
    }
    if unique_models.is_empty() {
        return Err(AppError::BadRequest(
            "at least one model is required; use '*' to accept any model".to_string(),
        ));
    }

    let mut mapping = HashMap::new();
    for (key, value) in model_mapping {
        let key = key.trim();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            mapping.insert(key.to_string(), value.to_string());
        }
    }

    for (key, value) in &extra_headers {
        HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| AppError::BadRequest(format!("invalid extra header name: {key}")))?;
        HeaderValue::from_str(value)
            .map_err(|_| AppError::BadRequest(format!("invalid extra header value for {key}")))?;
    }

    let proxy_url = proxy_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(proxy) = &proxy_url {
        if !(proxy.starts_with("http://")
            || proxy.starts_with("https://")
            || proxy.starts_with("socks5://")
            || proxy.starts_with("socks5h://"))
        {
            return Err(AppError::BadRequest(
                "proxy_url must start with http://, https://, socks5:// or socks5h://".to_string(),
            ));
        }
    }

    let timeout_ms = match timeout_ms {
        Some(value) if value <= 0 => {
            return Err(AppError::BadRequest(
                "timeout_ms must be positive".to_string(),
            ))
        }
        value => value,
    };

    let priority = priority.unwrap_or(0);
    if !(-10_000..=10_000).contains(&priority) {
        return Err(AppError::BadRequest(
            "priority must be between -10000 and 10000".to_string(),
        ));
    }

    let weight = weight.unwrap_or(1);
    if !(1..=1000).contains(&weight) {
        return Err(AppError::BadRequest(
            "weight must be between 1 and 1000".to_string(),
        ));
    }

    Ok(ValidatedChannel {
        name,
        kinds: unique_kinds,
        fallback_kind,
        base_url,
        api_key,
        auth_header,
        models: unique_models,
        model_mapping: mapping,
        extra_headers,
        proxy_url,
        priority,
        weight,
        timeout_ms,
        enabled: if enabled.unwrap_or(true) { 1 } else { 0 },
    })
}

fn to_write(validated: ValidatedChannel, api_key_enc: String) -> Result<ChannelWrite, AppError> {
    Ok(ChannelWrite {
        name: validated.name,
        kind: validated.kinds[0].as_str().to_string(),
        kinds: serde_json::to_string(&validated.kinds)
            .map_err(|error| AppError::Internal(error.into()))?,
        fallback_kind: validated.fallback_kind.as_str().to_string(),
        base_url: validated.base_url,
        api_key_enc,
        auth_header: validated.auth_header,
        models: serde_json::to_string(&validated.models)
            .map_err(|error| AppError::Internal(error.into()))?,
        model_mapping: serde_json::to_string(&validated.model_mapping)
            .map_err(|error| AppError::Internal(error.into()))?,
        extra_headers: serde_json::to_string(&validated.extra_headers)
            .map_err(|error| AppError::Internal(error.into()))?,
        proxy_url: validated.proxy_url,
        priority: validated.priority,
        weight: validated.weight,
        timeout_ms: validated.timeout_ms,
        enabled: validated.enabled,
    })
}

pub async fn list(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Vec<ChannelOutput>>, AppError> {
    let channels = models::channel::list(&state.db).await?;
    Ok(Json(
        channels
            .into_iter()
            .map(|channel| output(channel, state.crypto.as_ref()))
            .collect(),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
) -> Result<Json<ChannelOutput>, AppError> {
    let channel = models::channel::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?;
    Ok(Json(output(channel, state.crypto.as_ref())))
}

pub async fn create(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<ChannelInput>,
) -> Result<(StatusCode, Json<ChannelOutput>), AppError> {
    let validated = validate(input, true)?;
    let api_key = validated
        .api_key
        .clone()
        .ok_or_else(|| AppError::BadRequest("api_key is required".to_string()))?;
    let api_key_enc = state.crypto.encrypt(&api_key).map_err(AppError::Internal)?;
    let write = to_write(validated, api_key_enc)?;
    let channel = models::channel::create(&state.db, &write, models::now_ms()).await?;
    state.cache.invalidate_channels();

    Ok((
        StatusCode::CREATED,
        Json(output(channel, state.crypto.as_ref())),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
    Json(input): Json<ChannelInput>,
) -> Result<Json<ChannelOutput>, AppError> {
    let existing = models::channel::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?;

    let validated = validate(input, false)?;
    let api_key_enc = match &validated.api_key {
        Some(api_key) => state.crypto.encrypt(api_key).map_err(AppError::Internal)?,
        None => existing.api_key_enc,
    };
    let write = to_write(validated, api_key_enc)?;

    if !models::channel::update(&state.db, id, &write, models::now_ms()).await? {
        return Err(AppError::NotFound("channel not found".to_string()));
    }
    state.cache.invalidate_channels();
    state.clients.clear();

    let channel = models::channel::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?;
    Ok(Json(output(channel, state.crypto.as_ref())))
}

pub async fn delete(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if !models::channel::delete(&state.db, id).await? {
        return Err(AppError::NotFound("channel not found".to_string()));
    }
    state.cache.invalidate_channels();
    state.clients.clear();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ChannelProbeInput {
    /// Set when testing an existing channel so a blank api_key keeps the stored one.
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(flatten)]
    pub channel: ChannelInput,
}

pub async fn test(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let channel = models::channel::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?;
    Ok(Json(run_test(&state, &channel).await?))
}

pub async fn fetch_models(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let channel = models::channel::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?;
    Ok(Json(run_fetch_models(&state, &channel).await?))
}

pub async fn probe_test(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<ChannelProbeInput>,
) -> Result<Json<Value>, AppError> {
    let channel = probe_channel(&state, input).await?;
    Ok(Json(run_test(&state, &channel).await?))
}

pub async fn probe_fetch_models(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<ChannelProbeInput>,
) -> Result<Json<Value>, AppError> {
    let channel = probe_channel(&state, input).await?;
    Ok(Json(run_fetch_models(&state, &channel).await?))
}

/// Build a transient channel from an unsaved form so the connectivity test and
/// model fetch work before the channel is persisted.
async fn probe_channel(state: &AppState, input: ChannelProbeInput) -> Result<Channel, AppError> {
    let creating = input.id.is_none();
    let validated = validate(input.channel, creating)?;
    let api_key_enc = match &validated.api_key {
        Some(api_key) => state.crypto.encrypt(api_key).map_err(AppError::Internal)?,
        None => {
            let id = input.id.ok_or_else(|| {
                AppError::BadRequest("api_key is required when creating a channel".to_string())
            })?;
            models::channel::get(&state.db, id)
                .await?
                .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?
                .api_key_enc
        }
    };
    let now = models::now_ms();

    Ok(Channel {
        id: input.id.unwrap_or(0),
        name: validated.name,
        kind: validated.kinds[0].as_str().to_string(),
        kinds: serde_json::to_string(&validated.kinds)
            .map_err(|error| AppError::Internal(error.into()))?,
        fallback_kind: Some(validated.fallback_kind.as_str().to_string()),
        base_url: validated.base_url,
        api_key_enc,
        auth_header: validated.auth_header,
        models: serde_json::to_string(&validated.models)
            .map_err(|error| AppError::Internal(error.into()))?,
        model_mapping: serde_json::to_string(&validated.model_mapping)
            .map_err(|error| AppError::Internal(error.into()))?,
        extra_headers: serde_json::to_string(&validated.extra_headers)
            .map_err(|error| AppError::Internal(error.into()))?,
        proxy_url: validated.proxy_url,
        priority: validated.priority,
        weight: validated.weight,
        timeout_ms: validated.timeout_ms,
        enabled: validated.enabled,
        last_error: None,
        last_error_at: None,
        disabled_until: None,
        consecutive_failures: 0,
        created_at: now,
        updated_at: now,
    })
}

async fn run_test(state: &AppState, channel: &Channel) -> Result<Value, AppError> {
    let model = first_test_model(channel).ok_or_else(|| {
        AppError::BadRequest(
            "channel has no concrete model to test; add a model name first".to_string(),
        )
    })?;

    let started = Instant::now();
    let body = test_body(&channel.kind, &model);
    let result = send_test(state, channel, body).await;

    Ok(match result {
        Ok((status, text)) => json!({
            "ok": status.is_success(),
            "status": status.as_u16(),
            "latency_ms": started.elapsed().as_millis() as i64,
            "message": text.chars().take(800).collect::<String>(),
        }),
        Err(error) => json!({
            "ok": false,
            "status": 0,
            "latency_ms": started.elapsed().as_millis() as i64,
            "message": error.to_string(),
        }),
    })
}

async fn run_fetch_models(state: &AppState, channel: &Channel) -> Result<Value, AppError> {
    let started = Instant::now();
    let result = send_models(state, channel).await;

    Ok(match result {
        Ok((status, text)) => {
            let models = parse_models(&text);
            json!({
                "ok": status.is_success(),
                "status": status.as_u16(),
                "latency_ms": started.elapsed().as_millis() as i64,
                "models": models,
                "message": if models.is_empty() {
                    text.chars().take(800).collect::<String>()
                } else {
                    String::new()
                },
            })
        }
        Err(error) => json!({
            "ok": false,
            "status": 0,
            "latency_ms": started.elapsed().as_millis() as i64,
            "models": [],
            "message": error.to_string(),
        }),
    })
}

fn first_test_model(channel: &Channel) -> Option<String> {
    let models: Vec<String> = serde_json::from_str(&channel.models).ok()?;
    let model = models
        .into_iter()
        .find(|model| !model.is_empty() && model != "*")?;
    let mapping: HashMap<String, String> =
        serde_json::from_str(&channel.model_mapping).unwrap_or_default();
    Some(mapping.get(&model).cloned().unwrap_or(model))
}

fn test_body(kind: &str, model: &str) -> Value {
    match kind {
        "openai_responses" => json!({
            "model": model,
            "input": "ping",
            "max_output_tokens": 16,
            "store": false,
        }),
        "anthropic" => json!({
            "model": model,
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}],
        }),
        _ => json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
            "stream": false,
        }),
    }
}

async fn send_test(
    state: &AppState,
    channel: &Channel,
    body: Value,
) -> anyhow::Result<(StatusCode, String)> {
    let defaults = state.cache.defaults(&state.db).await?;
    let client = upstream::build_client(channel, defaults.nonstream_timeout_ms)?;
    let request = upstream::build_request(
        &client,
        channel,
        &state.crypto,
        &channel.kind,
        &body,
        &HeaderMap::new(),
    )?;
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    Ok((status, text))
}

async fn send_models(state: &AppState, channel: &Channel) -> anyhow::Result<(StatusCode, String)> {
    let defaults = state.cache.defaults(&state.db).await?;
    let client = upstream::build_client(channel, defaults.nonstream_timeout_ms)?;
    let request = upstream::apply_auth(
        client.get(upstream::models_url(channel)),
        channel,
        &state.crypto,
        &channel.kind,
        &HeaderMap::new(),
    )?;
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    Ok((status, text))
}

fn parse_models(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };

    let mut result = Vec::new();
    for key in ["data", "models"] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(id) = item
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| item.as_str())
                {
                    result.push(id.to_string());
                }
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_body_matches_protocol() {
        let chat = test_body("openai_chat", "gpt-test");
        assert_eq!(chat["model"], json!("gpt-test"));
        assert_eq!(chat["max_tokens"], json!(1));

        let responses = test_body("openai_responses", "gpt-test");
        assert_eq!(responses["max_output_tokens"], json!(16));
        assert_eq!(responses["store"], json!(false));

        let anthropic = test_body("anthropic", "claude-test");
        assert_eq!(anthropic["max_tokens"], json!(16));
        assert_eq!(anthropic["messages"][0]["role"], json!("user"));
    }

    #[test]
    fn parse_models_accepts_openai_and_anthropic_shapes() {
        let openai = parse_models(r#"{"data":[{"id":"gpt-b"},{"id":"gpt-a"}]}"#);
        assert_eq!(openai, vec!["gpt-a", "gpt-b"]);

        let anthropic = parse_models(r#"{"data":[{"id":"claude-b"}],"has_more":false}"#);
        assert_eq!(anthropic, vec!["claude-b"]);

        let alt = parse_models(r#"{"models":[{"id":"m1"},"m2"]}"#);
        assert_eq!(alt, vec!["m1", "m2"]);

        assert!(parse_models("not json").is_empty());
    }
}
