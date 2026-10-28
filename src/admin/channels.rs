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
        channel::{
            self, Channel, ChannelWrite, ProtocolConfig, ProtocolMode, ResolvedRoute, PROTOCOLS,
        },
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
        channel::default_auth_header(self.as_str())
    }
}

#[derive(Debug, Deserialize)]
pub struct ProtocolInput {
    pub protocol: ChannelKind,
    #[serde(default)]
    pub mode: ProtocolMode,
    #[serde(default)]
    pub target: Option<ChannelKind>,
    #[serde(default)]
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
}

#[derive(Debug, Deserialize)]
pub struct ChannelInput {
    pub name: String,
    /// New per-protocol layout. When empty, the legacy flat fields below are
    /// translated into the same structure for backward compatibility.
    #[serde(default)]
    pub protocols: Vec<ProtocolInput>,
    #[serde(default)]
    pub kind: Option<ChannelKind>,
    #[serde(default)]
    pub kinds: Vec<ChannelKind>,
    #[serde(default)]
    pub fallback_kind: Option<ChannelKind>,
    #[serde(default)]
    pub base_url: Option<String>,
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
pub struct ProtocolOutput {
    pub protocol: ChannelKind,
    pub mode: ProtocolMode,
    pub target: Option<ChannelKind>,
    pub base_url: String,
    pub auth_header: String,
    pub api_key_masked: String,
    pub models: Vec<String>,
    pub model_mapping: HashMap<String, String>,
    pub extra_headers: HashMap<String, String>,
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
    pub protocols: Vec<ProtocolOutput>,
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

fn mask_key(crypto: &Crypto, encoded: &str) -> String {
    if encoded.is_empty() {
        String::new()
    } else {
        crypto.mask(encoded)
    }
}

fn output(channel: Channel, crypto: &Crypto) -> ChannelOutput {
    let configs = channel::parse_protocol_configs(&channel);
    let natives: Vec<&ProtocolConfig> = configs
        .iter()
        .filter(|config| config.mode == ProtocolMode::Native)
        .collect();
    let primary = natives.first();

    let kinds: Vec<ChannelKind> = natives
        .iter()
        .map(|config| ChannelKind::from_db(&config.protocol))
        .collect();
    let fallback_kind = channel
        .fallback_kind
        .as_deref()
        .map(ChannelKind::from_db)
        .filter(|kind| kinds.contains(kind))
        .or_else(|| kinds.first().copied())
        .unwrap_or_else(|| ChannelKind::from_db(&channel.kind));

    let protocols = configs
        .iter()
        .map(|config| ProtocolOutput {
            protocol: ChannelKind::from_db(&config.protocol),
            mode: config.mode,
            target: config.target.as_deref().map(ChannelKind::from_db),
            base_url: config.base_url.clone(),
            auth_header: config.auth_header.clone(),
            api_key_masked: mask_key(crypto, &config.api_key_enc),
            models: config.models.clone(),
            model_mapping: config.model_mapping.clone(),
            extra_headers: config.extra_headers.clone(),
        })
        .collect();

    ChannelOutput {
        id: channel.id,
        name: channel.name,
        kind: kinds.first().copied().unwrap_or(fallback_kind),
        kinds,
        fallback_kind,
        base_url: primary
            .map(|config| config.base_url.clone())
            .unwrap_or(channel.base_url),
        auth_header: primary
            .map(|config| config.auth_header.clone())
            .unwrap_or(channel.auth_header),
        api_key_masked: primary
            .map(|config| mask_key(crypto, &config.api_key_enc))
            .unwrap_or_else(|| mask_key(crypto, &channel.api_key_enc)),
        models: primary
            .map(|config| config.models.clone())
            .unwrap_or_else(|| serde_json::from_str(&channel.models).unwrap_or_default()),
        model_mapping: primary
            .map(|config| config.model_mapping.clone())
            .unwrap_or_else(|| serde_json::from_str(&channel.model_mapping).unwrap_or_default()),
        extra_headers: primary
            .map(|config| config.extra_headers.clone())
            .unwrap_or_else(|| serde_json::from_str(&channel.extra_headers).unwrap_or_default()),
        protocols,
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
    protocols: Vec<ProtocolConfig>,
    legacy_fallback: Option<String>,
    proxy_url: Option<String>,
    priority: i64,
    weight: i64,
    timeout_ms: Option<i64>,
    enabled: i64,
}

fn validate_base_url(value: &str) -> Result<String, AppError> {
    let base_url = value.trim().trim_end_matches('/').to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(AppError::BadRequest(
            "base_url must start with http:// or https://".to_string(),
        ));
    }
    Ok(base_url)
}

fn normalize_auth_header(value: Option<String>, kind: ChannelKind) -> Result<String, AppError> {
    let auth_header = value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| kind.default_auth_header().to_string());
    if auth_header != "bearer" && auth_header != "x-api-key" {
        return Err(AppError::BadRequest(
            "auth_header must be 'bearer' or 'x-api-key'".to_string(),
        ));
    }
    Ok(auth_header)
}

fn normalize_models(models: Vec<String>, required: bool) -> Result<Vec<String>, AppError> {
    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for model in models {
        let model = model.trim();
        if !model.is_empty() && seen.insert(model.to_string()) {
            unique.push(model.to_string());
        }
    }
    if required && unique.is_empty() {
        return Err(AppError::BadRequest(
            "at least one model is required; use '*' to accept any model".to_string(),
        ));
    }
    Ok(unique)
}

fn normalize_mapping(mapping: HashMap<String, String>) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for (key, value) in mapping {
        let key = key.trim();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            result.insert(key.to_string(), value.to_string());
        }
    }
    result
}

fn validate_headers(extra_headers: &HashMap<String, String>) -> Result<(), AppError> {
    for (key, value) in extra_headers {
        HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| AppError::BadRequest(format!("invalid extra header name: {key}")))?;
        HeaderValue::from_str(value)
            .map_err(|_| AppError::BadRequest(format!("invalid extra header value for {key}")))?;
    }
    Ok(())
}

fn existing_protocol(existing: Option<&Channel>, kind: ChannelKind) -> Option<ProtocolConfig> {
    existing
        .map(channel::parse_protocol_configs)
        .and_then(|configs| {
            configs
                .into_iter()
                .find(|config| config.protocol == kind.as_str())
        })
}

fn resolve_key(
    input: Option<String>,
    existing: Option<&Channel>,
    kind: ChannelKind,
    crypto: &Crypto,
) -> Result<String, AppError> {
    let plain = input
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(plain) = plain {
        return crypto.encrypt(&plain).map_err(AppError::Internal);
    }

    if let Some(config) = existing_protocol(existing, kind) {
        if !config.api_key_enc.is_empty() {
            return Ok(config.api_key_enc);
        }
    }

    Err(AppError::BadRequest(format!(
        "api_key is required for protocol {}",
        kind.as_str()
    )))
}

fn validate_protocols(
    inputs: Vec<ProtocolInput>,
    existing: Option<&Channel>,
    crypto: &Crypto,
) -> Result<Vec<ProtocolConfig>, AppError> {
    let mut by_kind: HashMap<ChannelKind, ProtocolInput> = HashMap::new();
    for input in inputs {
        if by_kind.insert(input.protocol, input).is_some() {
            return Err(AppError::BadRequest(
                "duplicate protocol entries".to_string(),
            ));
        }
    }

    let mut configs: Vec<ProtocolConfig> = Vec::new();
    for kind in [
        ChannelKind::OpenaiChat,
        ChannelKind::OpenaiResponses,
        ChannelKind::Anthropic,
    ] {
        let Some(input) = by_kind.remove(&kind) else {
            continue;
        };

        let auth_header = normalize_auth_header(input.auth_header, kind)?;
        let model_mapping = normalize_mapping(input.model_mapping);
        validate_headers(&input.extra_headers)?;

        match input.mode {
            ProtocolMode::Native => {
                let base_url = validate_base_url(&input.base_url)?;
                let models = normalize_models(input.models, true)?;
                let api_key_enc = resolve_key(input.api_key, existing, kind, crypto)?;
                configs.push(ProtocolConfig {
                    protocol: kind.as_str().to_string(),
                    mode: ProtocolMode::Native,
                    target: None,
                    base_url,
                    api_key_enc,
                    auth_header,
                    models,
                    model_mapping,
                    extra_headers: input.extra_headers,
                });
            }
            ProtocolMode::Convert => {
                let target = input.target.ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "conversion target is required for protocol {}",
                        kind.as_str()
                    ))
                })?;
                if target == kind {
                    return Err(AppError::BadRequest(
                        "conversion target cannot be the same protocol".to_string(),
                    ));
                }
                let base_url = if input.base_url.trim().is_empty() {
                    String::new()
                } else {
                    validate_base_url(&input.base_url)?
                };
                let api_key_enc = match input.api_key {
                    Some(value) if !value.trim().is_empty() => {
                        crypto.encrypt(value.trim()).map_err(AppError::Internal)?
                    }
                    _ => existing_protocol(existing, kind)
                        .map(|config| config.api_key_enc)
                        .unwrap_or_default(),
                };
                configs.push(ProtocolConfig {
                    protocol: kind.as_str().to_string(),
                    mode: ProtocolMode::Convert,
                    target: Some(target.as_str().to_string()),
                    base_url,
                    api_key_enc,
                    auth_header,
                    models: normalize_models(input.models, false)?,
                    model_mapping,
                    extra_headers: input.extra_headers,
                });
            }
        }
    }

    let native_kinds: Vec<ChannelKind> = configs
        .iter()
        .filter(|config| config.mode == ProtocolMode::Native)
        .map(|config| ChannelKind::from_db(&config.protocol))
        .collect();
    if native_kinds.is_empty() {
        return Err(AppError::BadRequest(
            "at least one protocol must use its native upstream".to_string(),
        ));
    }

    for config in &configs {
        if config.mode != ProtocolMode::Convert {
            continue;
        }
        let target = config
            .target
            .as_deref()
            .and_then(|target| {
                PROTOCOLS
                    .iter()
                    .copied()
                    .find(|kind| *kind == target)
                    .map(ChannelKind::from_db)
            })
            .ok_or_else(|| AppError::BadRequest("invalid conversion target".to_string()))?;
        if !native_kinds.contains(&target) {
            return Err(AppError::BadRequest(
                "conversion target must be a protocol with its native upstream".to_string(),
            ));
        }
    }

    let first_native = native_kinds[0];
    for kind in [
        ChannelKind::OpenaiChat,
        ChannelKind::OpenaiResponses,
        ChannelKind::Anthropic,
    ] {
        if configs
            .iter()
            .any(|config| config.protocol == kind.as_str())
        {
            continue;
        }
        configs.push(ProtocolConfig {
            protocol: kind.as_str().to_string(),
            mode: ProtocolMode::Convert,
            target: Some(first_native.as_str().to_string()),
            base_url: String::new(),
            api_key_enc: existing_protocol(existing, kind)
                .map(|config| config.api_key_enc)
                .unwrap_or_default(),
            auth_header: kind.default_auth_header().to_string(),
            models: Vec::new(),
            model_mapping: HashMap::new(),
            extra_headers: HashMap::new(),
        });
    }

    configs.sort_by_key(|config| {
        PROTOCOLS
            .iter()
            .position(|kind| *kind == config.protocol)
            .unwrap_or(PROTOCOLS.len())
    });
    Ok(configs)
}

struct LegacyProtocolsInput {
    kinds: Vec<ChannelKind>,
    fallback_kind: Option<ChannelKind>,
    base_url: String,
    api_key: Option<String>,
    auth_header: Option<String>,
    models: Vec<String>,
    model_mapping: HashMap<String, String>,
    extra_headers: HashMap<String, String>,
}

fn legacy_protocols(
    input: LegacyProtocolsInput,
    existing: Option<&Channel>,
    crypto: &Crypto,
) -> Result<(Vec<ProtocolConfig>, String), AppError> {
    let LegacyProtocolsInput {
        kinds,
        fallback_kind,
        base_url,
        api_key,
        auth_header,
        models,
        model_mapping,
        extra_headers,
    } = input;
    let mut unique_kinds = Vec::new();
    for kind in kinds {
        if !unique_kinds.contains(&kind) {
            unique_kinds.push(kind);
        }
    }
    if unique_kinds.is_empty() {
        return Err(AppError::BadRequest(
            "at least one channel protocol is required".to_string(),
        ));
    }
    let fallback = fallback_kind.unwrap_or(unique_kinds[0]);
    if !unique_kinds.contains(&fallback) {
        return Err(AppError::BadRequest(
            "fallback_kind must be one of the configured protocols".to_string(),
        ));
    }

    let base_url = validate_base_url(&base_url)?;
    let auth_header = normalize_auth_header(auth_header, unique_kinds[0])?;
    let models = normalize_models(models, true)?;
    let model_mapping = normalize_mapping(model_mapping);
    validate_headers(&extra_headers)?;

    let plain = api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let api_key_enc = match plain {
        Some(plain) => crypto.encrypt(&plain).map_err(AppError::Internal)?,
        None => existing
            .map(|channel| channel.api_key_enc.clone())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                AppError::BadRequest("api_key is required when creating a channel".to_string())
            })?,
    };

    let kinds_json = serde_json::to_string(
        &unique_kinds
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| AppError::Internal(error.into()))?;
    let models_json =
        serde_json::to_string(&models).map_err(|error| AppError::Internal(error.into()))?;
    let mapping_json =
        serde_json::to_string(&model_mapping).map_err(|error| AppError::Internal(error.into()))?;
    let headers_json =
        serde_json::to_string(&extra_headers).map_err(|error| AppError::Internal(error.into()))?;

    Ok((
        channel::legacy_protocol_configs(channel::LegacyConfigSource {
            kind: unique_kinds[0].as_str(),
            kinds_json: &kinds_json,
            fallback_kind: Some(fallback.as_str()),
            base_url: &base_url,
            api_key_enc: &api_key_enc,
            auth_header: &auth_header,
            models_json: &models_json,
            model_mapping_json: &mapping_json,
            extra_headers_json: &headers_json,
        }),
        fallback.as_str().to_string(),
    ))
}

fn validate(
    input: ChannelInput,
    creating: bool,
    existing: Option<&Channel>,
    crypto: &Crypto,
) -> Result<ValidatedChannel, AppError> {
    let ChannelInput {
        name,
        protocols,
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

    let (protocols, legacy_fallback) = if protocols.is_empty() {
        let (configs, fallback) = legacy_protocols(
            LegacyProtocolsInput {
                kinds: if kinds.is_empty() {
                    kind.into_iter().collect()
                } else {
                    kinds
                },
                fallback_kind,
                base_url: base_url.unwrap_or_default(),
                api_key,
                auth_header,
                models,
                model_mapping,
                extra_headers,
            },
            existing,
            crypto,
        )?;
        (configs, Some(fallback))
    } else {
        (validate_protocols(protocols, existing, crypto)?, None)
    };

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

    // Creating still requires every native route to have a key; updates may
    // reuse the stored encrypted key for each protocol.
    if creating {
        for config in protocols
            .iter()
            .filter(|config| config.mode == ProtocolMode::Native)
        {
            if config.api_key_enc.is_empty() {
                return Err(AppError::BadRequest(format!(
                    "api_key is required for protocol {} when creating a channel",
                    config.protocol
                )));
            }
        }
    }

    Ok(ValidatedChannel {
        name,
        protocols,
        legacy_fallback,
        proxy_url,
        priority,
        weight,
        timeout_ms,
        enabled: if enabled.unwrap_or(true) { 1 } else { 0 },
    })
}

fn to_write(validated: ValidatedChannel) -> Result<ChannelWrite, AppError> {
    let primary = validated
        .protocols
        .iter()
        .find(|config| config.mode == ProtocolMode::Native)
        .ok_or_else(|| {
            AppError::BadRequest("at least one native protocol is required".to_string())
        })?;
    let native_kinds: Vec<&str> = validated
        .protocols
        .iter()
        .filter(|config| config.mode == ProtocolMode::Native)
        .map(|config| config.protocol.as_str())
        .collect();
    let fallback_kind = validated
        .legacy_fallback
        .filter(|kind| native_kinds.contains(&kind.as_str()))
        .unwrap_or_else(|| primary.protocol.clone());

    Ok(ChannelWrite {
        name: validated.name,
        kind: primary.protocol.clone(),
        kinds: serde_json::to_string(&native_kinds)
            .map_err(|error| AppError::Internal(error.into()))?,
        fallback_kind,
        protocol_configs: serde_json::to_string(&validated.protocols)
            .map_err(|error| AppError::Internal(error.into()))?,
        base_url: primary.base_url.clone(),
        api_key_enc: primary.api_key_enc.clone(),
        auth_header: primary.auth_header.clone(),
        models: serde_json::to_string(&primary.models)
            .map_err(|error| AppError::Internal(error.into()))?,
        model_mapping: serde_json::to_string(&primary.model_mapping)
            .map_err(|error| AppError::Internal(error.into()))?,
        extra_headers: serde_json::to_string(&primary.extra_headers)
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
    let validated = validate(input, true, None, state.crypto.as_ref())?;
    let write = to_write(validated)?;
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

    let validated = validate(input, false, Some(&existing), state.crypto.as_ref())?;
    let write = to_write(validated)?;

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
    /// Protocol tab being tested. Defaults to the first native route.
    #[serde(default)]
    pub probe_protocol: Option<ChannelKind>,
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
    let route = primary_route(&channel)?;
    Ok(Json(run_test(&state, &channel, &route).await?))
}

pub async fn fetch_models(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let channel = models::channel::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?;
    let route = primary_route(&channel)?;
    Ok(Json(run_fetch_models(&state, &channel, &route).await?))
}

pub async fn probe_test(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<ChannelProbeInput>,
) -> Result<Json<Value>, AppError> {
    let (channel, route) = probe_channel(&state, input).await?;
    Ok(Json(run_test(&state, &channel, &route).await?))
}

pub async fn probe_fetch_models(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<ChannelProbeInput>,
) -> Result<Json<Value>, AppError> {
    let (channel, route) = probe_channel(&state, input).await?;
    Ok(Json(run_fetch_models(&state, &channel, &route).await?))
}

fn primary_route(channel: &Channel) -> Result<ResolvedRoute, AppError> {
    let primary = channel::native_configs(channel)
        .into_iter()
        .next()
        .ok_or_else(|| {
            AppError::BadRequest("channel has no native protocol configured".to_string())
        })?;
    channel::resolve_route(channel, &primary.protocol)
        .ok_or_else(|| AppError::BadRequest("channel has no routable native protocol".to_string()))
}

/// Build a transient channel from an unsaved form so the connectivity test and
/// model fetch work before the channel is persisted.
async fn probe_channel(
    state: &AppState,
    input: ChannelProbeInput,
) -> Result<(Channel, ResolvedRoute), AppError> {
    let ChannelProbeInput {
        id,
        probe_protocol,
        channel: channel_input,
    } = input;

    let existing = match id {
        Some(id) => Some(
            models::channel::get(&state.db, id)
                .await?
                .ok_or_else(|| AppError::NotFound("channel not found".to_string()))?,
        ),
        None => None,
    };

    let creating = existing.is_none();
    let validated = validate(
        channel_input,
        creating,
        existing.as_ref(),
        state.crypto.as_ref(),
    )?;
    let write = to_write(validated)?;
    let now = models::now_ms();
    let transient = Channel {
        id: id.unwrap_or(0),
        name: write.name.clone(),
        kind: write.kind.clone(),
        kinds: write.kinds.clone(),
        fallback_kind: Some(write.fallback_kind.clone()),
        protocol_configs: write.protocol_configs.clone(),
        base_url: write.base_url.clone(),
        api_key_enc: write.api_key_enc.clone(),
        auth_header: write.auth_header.clone(),
        models: write.models.clone(),
        model_mapping: write.model_mapping.clone(),
        extra_headers: write.extra_headers.clone(),
        proxy_url: write.proxy_url.clone(),
        priority: write.priority,
        weight: write.weight,
        timeout_ms: write.timeout_ms,
        enabled: write.enabled,
        last_error: None,
        last_error_at: None,
        disabled_until: None,
        consecutive_failures: 0,
        created_at: now,
        updated_at: now,
    };

    let probe_kind = match probe_protocol {
        Some(kind) => kind.as_str().to_string(),
        None => channel::native_configs(&transient)
            .into_iter()
            .next()
            .map(|config| config.protocol)
            .ok_or_else(|| AppError::BadRequest("channel has no protocol to test".to_string()))?,
    };

    let route = channel::resolve_route(&transient, &probe_kind).ok_or_else(|| {
        AppError::BadRequest(format!(
            "protocol {probe_kind} has no routable upstream configuration"
        ))
    })?;

    Ok((transient, route))
}

async fn run_test(
    state: &AppState,
    channel: &Channel,
    route: &ResolvedRoute,
) -> Result<Value, AppError> {
    let model = first_test_model(&route.config).ok_or_else(|| {
        AppError::BadRequest(
            "channel has no concrete model to test; add a model name first".to_string(),
        )
    })?;

    let started = Instant::now();
    let body = test_body(&route.upstream_kind, &model);
    let result = send_test(state, channel, route, body).await;

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

async fn run_fetch_models(
    state: &AppState,
    channel: &Channel,
    route: &ResolvedRoute,
) -> Result<Value, AppError> {
    let started = Instant::now();
    let result = send_models(state, channel, route).await;

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

fn first_test_model(config: &ProtocolConfig) -> Option<String> {
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
    route: &ResolvedRoute,
    body: Value,
) -> anyhow::Result<(StatusCode, String)> {
    let defaults = state.cache.defaults(&state.db).await?;
    let client = upstream::build_client(channel, defaults.nonstream_timeout_ms)?;
    let request = upstream::build_request(
        &client,
        &route.config,
        &state.crypto,
        &route.upstream_kind,
        &body,
        &HeaderMap::new(),
    )?;
    let response = request.send().await?;
    let status = response.status();
    let text = upstream::read_body_limited(response)
        .await
        .unwrap_or_default();
    Ok((status, text))
}

async fn send_models(
    state: &AppState,
    channel: &Channel,
    route: &ResolvedRoute,
) -> anyhow::Result<(StatusCode, String)> {
    let defaults = state.cache.defaults(&state.db).await?;
    let client = upstream::build_client(channel, defaults.nonstream_timeout_ms)?;
    let request = upstream::apply_auth(
        client.get(upstream::models_url(&route.config)),
        &route.config,
        &state.crypto,
        &route.upstream_kind,
        &HeaderMap::new(),
    )?;
    let response = request.send().await?;
    let status = response.status();
    let text = upstream::read_body_limited(response)
        .await
        .unwrap_or_default();
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
