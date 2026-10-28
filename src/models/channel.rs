use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub kinds: String,
    pub fallback_kind: Option<String>,
    pub protocol_configs: String,
    pub base_url: String,
    pub api_key_enc: String,
    pub auth_header: String,
    pub models: String,
    pub model_mapping: String,
    pub extra_headers: String,
    pub proxy_url: Option<String>,
    pub priority: i64,
    pub weight: i64,
    pub timeout_ms: Option<i64>,
    pub enabled: i64,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    pub disabled_until: Option<i64>,
    pub consecutive_failures: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Channel {
    /// Whether this row may still carry failure state that a successful
    /// attempt has to clear. Healthy rows can skip the write entirely.
    pub fn has_failure_state(&self) -> bool {
        self.consecutive_failures != 0 || self.last_error.is_some() || self.disabled_until.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct ChannelWrite {
    pub name: String,
    pub kind: String,
    pub kinds: String,
    pub fallback_kind: String,
    pub protocol_configs: String,
    pub base_url: String,
    pub api_key_enc: String,
    pub auth_header: String,
    pub models: String,
    pub model_mapping: String,
    pub extra_headers: String,
    pub proxy_url: Option<String>,
    pub priority: i64,
    pub weight: i64,
    pub timeout_ms: Option<i64>,
    pub enabled: i64,
}

pub const PROTOCOLS: [&str; 3] = ["openai_chat", "openai_responses", "anthropic"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolMode {
    #[default]
    Native,
    Convert,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProtocolConfig {
    pub protocol: String,
    #[serde(default)]
    pub mode: ProtocolMode,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key_enc: String,
    #[serde(default)]
    pub auth_header: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub upstream_kind: String,
    pub config: ProtocolConfig,
}

#[derive(Debug, Clone, Default)]
pub struct CatalogModels {
    pub models: Vec<String>,
    pub has_wildcard: bool,
}

/// A channel with its per-protocol configuration already parsed. The runtime
/// cache prepares these once per refresh so a gateway request does not
/// re-parse the same JSON for every channel it considers.
#[derive(Debug, Clone)]
pub struct PreparedChannel {
    pub channel: Channel,
    configs: Vec<ProtocolConfig>,
    pub catalog: CatalogModels,
}

impl PreparedChannel {
    pub fn new(channel: Channel) -> Self {
        let configs = parse_protocol_configs(&channel);
        let catalog = catalog_models_from(&channel, &configs);
        Self {
            channel,
            configs,
            catalog,
        }
    }

    /// Route to a config index without cloning the configuration. The routing
    /// hot path only needs the upstream protocol name and the target config.
    pub fn resolve_route_ref(&self, inbound_kind: &str) -> Option<(&str, usize)> {
        let inbound_index = self
            .configs
            .iter()
            .position(|config| config.protocol == inbound_kind)?;
        let inbound = &self.configs[inbound_index];
        match inbound.mode {
            ProtocolMode::Native => {
                if inbound.base_url.is_empty() {
                    return None;
                }
                Some((inbound.protocol.as_str(), inbound_index))
            }
            ProtocolMode::Convert => {
                let target = inbound.target.as_deref()?;
                if target == inbound_kind {
                    return None;
                }
                let index = self.configs.iter().position(|config| {
                    config.protocol == target
                        && config.mode == ProtocolMode::Native
                        && !config.base_url.is_empty()
                })?;
                Some((self.configs[index].protocol.as_str(), index))
            }
        }
    }

    pub fn config_at(&self, index: usize) -> &ProtocolConfig {
        &self.configs[index]
    }
}

const SELECT_COLUMNS: &str =
    "id, name, kind, kinds, fallback_kind, protocol_configs, base_url, api_key_enc, auth_header, models,
    model_mapping, extra_headers, proxy_url, priority, weight, timeout_ms, enabled,
    last_error, last_error_at, disabled_until, consecutive_failures, created_at, updated_at";

pub fn is_known_kind(kind: &str) -> bool {
    PROTOCOLS.contains(&kind)
}

pub fn default_auth_header(kind: &str) -> &'static str {
    if kind == "anthropic" {
        "x-api-key"
    } else {
        "bearer"
    }
}

/// Map an inbound protocol name (`chat`/`responses`/`anthropic`) to the channel
/// kind string used in storage.
pub fn kind_for_protocol(protocol: &str) -> &'static str {
    match protocol {
        "responses" => "openai_responses",
        "anthropic" => "anthropic",
        _ => "openai_chat",
    }
}

/// Everything needed to synthesize per-protocol configs from the legacy single
/// configuration. Used both for the one-time backfill and for legacy API input.
#[derive(Debug, Clone, Copy)]
pub struct LegacyConfigSource<'a> {
    pub kind: &'a str,
    pub kinds_json: &'a str,
    pub fallback_kind: Option<&'a str>,
    pub base_url: &'a str,
    pub api_key_enc: &'a str,
    pub auth_header: &'a str,
    pub models_json: &'a str,
    pub model_mapping_json: &'a str,
    pub extra_headers_json: &'a str,
}

pub fn legacy_protocol_configs(source: LegacyConfigSource<'_>) -> Vec<ProtocolConfig> {
    let mut kinds = Vec::new();
    if let Ok(parsed) = serde_json::from_str::<Vec<String>>(source.kinds_json) {
        for kind in parsed {
            if is_known_kind(&kind) && !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    if kinds.is_empty() {
        kinds.push(if is_known_kind(source.kind) {
            source.kind.to_string()
        } else {
            "openai_chat".to_string()
        });
    }

    let fallback = source
        .fallback_kind
        .filter(|kind| kinds.iter().any(|item| item == kind))
        .unwrap_or(&kinds[0])
        .to_string();

    let models: Vec<String> = serde_json::from_str(source.models_json).unwrap_or_default();
    let model_mapping: HashMap<String, String> =
        serde_json::from_str(source.model_mapping_json).unwrap_or_default();
    let extra_headers: HashMap<String, String> =
        serde_json::from_str(source.extra_headers_json).unwrap_or_default();

    PROTOCOLS
        .iter()
        .map(|protocol| {
            let native = kinds.iter().any(|kind| kind == protocol);
            ProtocolConfig {
                protocol: protocol.to_string(),
                mode: if native {
                    ProtocolMode::Native
                } else {
                    ProtocolMode::Convert
                },
                target: (!native).then(|| fallback.clone()),
                base_url: source.base_url.to_string(),
                api_key_enc: source.api_key_enc.to_string(),
                auth_header: source.auth_header.to_string(),
                models: models.clone(),
                model_mapping: model_mapping.clone(),
                extra_headers: extra_headers.clone(),
            }
        })
        .collect()
}

fn legacy_source(channel: &Channel) -> LegacyConfigSource<'_> {
    LegacyConfigSource {
        kind: &channel.kind,
        kinds_json: &channel.kinds,
        fallback_kind: channel.fallback_kind.as_deref(),
        base_url: &channel.base_url,
        api_key_enc: &channel.api_key_enc,
        auth_header: &channel.auth_header,
        models_json: &channel.models,
        model_mapping_json: &channel.model_mapping,
        extra_headers_json: &channel.extra_headers,
    }
}

/// The three protocol configs in canonical order. Malformed or missing entries
/// are repaired so callers and the UI always see exactly three routes.
pub fn parse_protocol_configs(channel: &Channel) -> Vec<ProtocolConfig> {
    let parsed: Vec<ProtocolConfig> =
        serde_json::from_str(&channel.protocol_configs).unwrap_or_default();
    if parsed.is_empty() {
        return legacy_protocol_configs(legacy_source(channel));
    }

    let mut by_kind: HashMap<String, ProtocolConfig> = HashMap::new();
    for config in parsed {
        if is_known_kind(&config.protocol) {
            by_kind.entry(config.protocol.clone()).or_insert(config);
        }
    }

    let first_native = PROTOCOLS.iter().find(|protocol| {
        by_kind.get(**protocol).is_some_and(|config| {
            config.mode == ProtocolMode::Native && !config.base_url.is_empty()
        })
    });

    PROTOCOLS
        .iter()
        .map(|protocol| match by_kind.remove(*protocol) {
            Some(mut config) => {
                config.protocol = protocol.to_string();
                if config.auth_header.is_empty() {
                    config.auth_header = default_auth_header(protocol).to_string();
                }
                if config.mode == ProtocolMode::Convert {
                    let target_valid = config
                        .target
                        .as_deref()
                        .is_some_and(|target| is_known_kind(target) && target != *protocol);
                    if !target_valid {
                        config.target = first_native
                            .filter(|native| **native != *protocol)
                            .map(|native| native.to_string());
                    }
                }
                config
            }
            None => {
                let target = first_native
                    .filter(|native| **native != *protocol)
                    .map(|native| native.to_string());
                ProtocolConfig {
                    protocol: protocol.to_string(),
                    mode: if target.is_some() {
                        ProtocolMode::Convert
                    } else {
                        ProtocolMode::Native
                    },
                    target,
                    auth_header: default_auth_header(protocol).to_string(),
                    ..Default::default()
                }
            }
        })
        .collect()
}

/// Resolve the upstream route for an inbound protocol. `None` means the channel
/// cannot serve that protocol (unconfigured native route or invalid target).
pub fn resolve_route(channel: &Channel, inbound_kind: &str) -> Option<ResolvedRoute> {
    resolve_route_from(&parse_protocol_configs(channel), inbound_kind)
}

fn resolve_route_from(configs: &[ProtocolConfig], inbound_kind: &str) -> Option<ResolvedRoute> {
    let inbound = configs
        .iter()
        .find(|config| config.protocol == inbound_kind)?;
    match inbound.mode {
        ProtocolMode::Native => {
            if inbound.base_url.is_empty() {
                return None;
            }
            Some(ResolvedRoute {
                upstream_kind: inbound.protocol.clone(),
                config: inbound.clone(),
            })
        }
        ProtocolMode::Convert => {
            let target = inbound.target.as_deref()?;
            if target == inbound_kind {
                return None;
            }
            let target_config = configs.iter().find(|config| {
                config.protocol == target
                    && config.mode == ProtocolMode::Native
                    && !config.base_url.is_empty()
            })?;
            Some(ResolvedRoute {
                upstream_kind: target_config.protocol.clone(),
                config: target_config.clone(),
            })
        }
    }
}

pub fn native_configs(channel: &Channel) -> Vec<ProtocolConfig> {
    parse_protocol_configs(channel)
        .into_iter()
        .filter(|config| config.mode == ProtocolMode::Native)
        .collect()
}

/// Models advertised to clients: the union of models from the native routes.
/// Conversion routes reuse their target's models, so they are already covered.
#[cfg(test)]
pub fn catalog_models(channel: &Channel) -> CatalogModels {
    let configs = parse_protocol_configs(channel);
    catalog_models_from(channel, &configs)
}

fn catalog_models_from(channel: &Channel, configs: &[ProtocolConfig]) -> CatalogModels {
    let mut models = BTreeSet::new();
    let mut has_wildcard = false;

    for config in configs
        .iter()
        .filter(|config| config.mode == ProtocolMode::Native)
    {
        for model in &config.models {
            if model == "*" {
                has_wildcard = true;
            } else if !model.is_empty() {
                models.insert(model.clone());
            }
        }
    }

    if models.is_empty() && !has_wildcard {
        if let Ok(legacy) = serde_json::from_str::<Vec<String>>(&channel.models) {
            for model in legacy {
                if model == "*" {
                    has_wildcard = true;
                } else if !model.is_empty() {
                    models.insert(model);
                }
            }
        }
    }

    CatalogModels {
        models: models.into_iter().collect(),
        has_wildcard,
    }
}

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<Channel>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM channels ORDER BY priority DESC, id ASC");
    sqlx::query_as(&query).fetch_all(pool).await
}

pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Channel>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM channels WHERE id = ?");
    sqlx::query_as(&query).bind(id).fetch_optional(pool).await
}

pub async fn create(pool: &SqlitePool, input: &ChannelWrite, now: i64) -> sqlx::Result<Channel> {
    let result = sqlx::query(
        "INSERT INTO channels
         (name, kind, kinds, fallback_kind, protocol_configs, base_url, api_key_enc, auth_header, models,
          model_mapping, extra_headers, proxy_url, priority, weight, timeout_ms, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&input.name)
    .bind(&input.kind)
    .bind(&input.kinds)
    .bind(&input.fallback_kind)
    .bind(&input.protocol_configs)
    .bind(&input.base_url)
    .bind(&input.api_key_enc)
    .bind(&input.auth_header)
    .bind(&input.models)
    .bind(&input.model_mapping)
    .bind(&input.extra_headers)
    .bind(&input.proxy_url)
    .bind(input.priority)
    .bind(input.weight)
    .bind(input.timeout_ms)
    .bind(input.enabled)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    get(pool, result.last_insert_rowid())
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

pub async fn update(
    pool: &SqlitePool,
    id: i64,
    input: &ChannelWrite,
    now: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE channels SET
            name = ?, kind = ?, kinds = ?, fallback_kind = ?, protocol_configs = ?, base_url = ?,
            api_key_enc = ?, auth_header = ?, models = ?, model_mapping = ?, extra_headers = ?,
            proxy_url = ?, priority = ?, weight = ?, timeout_ms = ?, enabled = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&input.name)
    .bind(&input.kind)
    .bind(&input.kinds)
    .bind(&input.fallback_kind)
    .bind(&input.protocol_configs)
    .bind(&input.base_url)
    .bind(&input.api_key_enc)
    .bind(&input.auth_header)
    .bind(&input.models)
    .bind(&input.model_mapping)
    .bind(&input.extra_headers)
    .bind(&input.proxy_url)
    .bind(input.priority)
    .bind(input.weight)
    .bind(input.timeout_ms)
    .bind(input.enabled)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM channels WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Populate `protocol_configs` for rows created before the per-protocol layout.
/// Idempotent: only rows with an empty JSON array are read.
pub async fn backfill_protocol_configs(pool: &SqlitePool) -> sqlx::Result<()> {
    #[derive(FromRow)]
    struct LegacyRow {
        id: i64,
        kind: String,
        kinds: String,
        fallback_kind: Option<String>,
        base_url: String,
        api_key_enc: String,
        auth_header: String,
        models: String,
        model_mapping: String,
        extra_headers: String,
    }

    let rows = sqlx::query_as::<_, LegacyRow>(
        "SELECT id, kind, kinds, fallback_kind, base_url, api_key_enc, auth_header, models,
                model_mapping, extra_headers
         FROM channels
         WHERE protocol_configs IS NULL OR protocol_configs = '[]'",
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let configs = legacy_protocol_configs(LegacyConfigSource {
            kind: &row.kind,
            kinds_json: &row.kinds,
            fallback_kind: row.fallback_kind.as_deref(),
            base_url: &row.base_url,
            api_key_enc: &row.api_key_enc,
            auth_header: &row.auth_header,
            models_json: &row.models,
            model_mapping_json: &row.model_mapping,
            extra_headers_json: &row.extra_headers,
        });
        let encoded = serde_json::to_string(&configs).unwrap_or_else(|_| "[]".to_string());
        sqlx::query("UPDATE channels SET protocol_configs = ? WHERE id = ?")
            .bind(encoded)
            .bind(row.id)
            .execute(pool)
            .await?;
    }

    Ok(())
}

const MAX_ERROR_CHARS: usize = 1024;

fn truncate_error(error: &str) -> String {
    if error.chars().count() <= MAX_ERROR_CHARS {
        return error.to_string();
    }
    let mut truncated: String = error.chars().take(MAX_ERROR_CHARS).collect();
    truncated.push('…');
    truncated
}

/// Health fields a failed attempt changes, returned so the caller can update
/// its in-process routing view without reloading every channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelHealth {
    pub consecutive_failures: i64,
    pub disabled_until: Option<i64>,
}

/// Record a failed attempt. Once a channel has failed twice in a row it enters
/// cooldown; each further consecutive failure doubles the previous cooldown,
/// capped at 30 minutes. A single failure only records the error so a transient
/// upstream hiccup does not remove a channel from rotation.
pub async fn mark_failure(
    pool: &SqlitePool,
    id: i64,
    error: &str,
    now: i64,
) -> sqlx::Result<Option<ChannelHealth>> {
    let error = truncate_error(error);
    let row = sqlx::query_as::<_, (i64, Option<i64>)>(
        "UPDATE channels SET
            consecutive_failures = consecutive_failures + 1,
            last_error = ?,
            last_error_at = ?,
            updated_at = ?,
            disabled_until = CASE
                WHEN consecutive_failures + 1 >= 2 THEN
                    ? + min(1800000, 60000 * (1 << min(consecutive_failures - 1, 5)))
                ELSE disabled_until
            END
         WHERE id = ?
         RETURNING consecutive_failures, disabled_until",
    )
    .bind(&error)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.map(|(consecutive_failures, disabled_until)| ChannelHealth {
            consecutive_failures,
            disabled_until,
        }),
    )
}

/// Clear failure state after a successful attempt. Returns whether a row was
/// changed so callers can skip cache invalidation for healthy channels.
pub async fn mark_success(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE channels SET
            consecutive_failures = 0,
            last_error = NULL,
            last_error_at = NULL,
            disabled_until = NULL
         WHERE id = ?
           AND (consecutive_failures <> 0 OR last_error IS NOT NULL OR disabled_until IS NOT NULL)",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory database");
        sqlx::query(
            "CREATE TABLE channels (
                id INTEGER PRIMARY KEY,
                last_error TEXT,
                last_error_at INTEGER,
                disabled_until INTEGER,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("schema");
        sqlx::query("INSERT INTO channels (id, updated_at) VALUES (1, 0)")
            .execute(&pool)
            .await
            .expect("seed");
        pool
    }

    async fn state(pool: &SqlitePool) -> (i64, Option<i64>, Option<String>) {
        sqlx::query_as(
            "SELECT consecutive_failures, disabled_until, last_error FROM channels WHERE id = 1",
        )
        .fetch_one(pool)
        .await
        .expect("row")
    }

    #[tokio::test]
    async fn first_failure_records_error_without_cooldown() {
        let pool = pool().await;
        mark_failure(&pool, 1, "boom", 1_000).await.expect("mark");

        let (failures, disabled_until, last_error) = state(&pool).await;
        assert_eq!(failures, 1);
        assert_eq!(disabled_until, None);
        assert_eq!(last_error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn consecutive_failures_back_off_exponentially() {
        let pool = pool().await;
        let now = 10_000;

        mark_failure(&pool, 1, "boom", now).await.expect("first");
        assert_eq!(state(&pool).await.1, None);

        let expected = [
            60_000, 120_000, 240_000, 480_000, 960_000, 1_800_000, 1_800_000,
        ];
        for cooldown in expected {
            mark_failure(&pool, 1, "boom", now).await.expect("mark");
            assert_eq!(state(&pool).await.1, Some(now + cooldown));
        }
    }

    #[tokio::test]
    async fn success_clears_failure_state() {
        let pool = pool().await;
        mark_failure(&pool, 1, "boom", 1_000).await.expect("first");
        mark_failure(&pool, 1, "boom", 1_000).await.expect("second");

        assert!(mark_success(&pool, 1).await.expect("clear"));
        assert!(!mark_success(&pool, 1).await.expect("healthy"));

        let (failures, disabled_until, last_error) = state(&pool).await;
        assert_eq!(failures, 0);
        assert_eq!(disabled_until, None);
        assert_eq!(last_error, None);
    }

    fn config_channel(kinds: &str, fallback: Option<&str>, legacy_kind: &str) -> Channel {
        Channel {
            id: 1,
            name: "test".to_string(),
            kind: legacy_kind.to_string(),
            kinds: kinds.to_string(),
            fallback_kind: fallback.map(str::to_string),
            protocol_configs: "[]".to_string(),
            base_url: "http://localhost".to_string(),
            api_key_enc: String::new(),
            auth_header: "bearer".to_string(),
            models: r#"["m1"]"#.to_string(),
            model_mapping: r#"{"m1":"upstream-m1"}"#.to_string(),
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
            updated_at: 0,
        }
    }

    #[test]
    fn detects_failure_state_that_a_success_must_clear() {
        let mut channel = config_channel(r#"["openai_chat"]"#, None, "openai_chat");
        assert!(!channel.has_failure_state());

        channel.consecutive_failures = 1;
        assert!(channel.has_failure_state());

        channel.consecutive_failures = 0;
        channel.last_error = Some("boom".to_string());
        assert!(channel.has_failure_state());

        channel.last_error = None;
        channel.disabled_until = Some(1);
        assert!(channel.has_failure_state());
    }

    #[test]
    fn legacy_configs_mark_configured_kinds_native_and_convert_the_rest() {
        let channel = config_channel(
            r#"["openai_chat","anthropic"]"#,
            Some("openai_chat"),
            "openai_chat",
        );
        let configs = legacy_protocol_configs(legacy_source(&channel));

        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0].protocol, "openai_chat");
        assert_eq!(configs[0].mode, ProtocolMode::Native);
        assert_eq!(configs[1].protocol, "openai_responses");
        assert_eq!(configs[1].mode, ProtocolMode::Convert);
        assert_eq!(configs[1].target.as_deref(), Some("openai_chat"));
        assert_eq!(configs[2].mode, ProtocolMode::Native);
    }

    #[test]
    fn resolve_route_uses_the_native_config_or_the_target() {
        let channel = config_channel(
            r#"["openai_chat","anthropic"]"#,
            Some("openai_chat"),
            "openai_chat",
        );

        let chat = resolve_route(&channel, "openai_chat").expect("chat route");
        assert_eq!(chat.upstream_kind, "openai_chat");
        assert_eq!(chat.config.models, vec!["m1"]);

        let responses = resolve_route(&channel, "openai_responses").expect("converted route");
        assert_eq!(responses.upstream_kind, "openai_chat");
        assert_eq!(responses.config.model_mapping["m1"], "upstream-m1");
    }

    #[test]
    fn resolve_route_rejects_convert_targets_that_are_not_native() {
        let channel = config_channel(r#"["openai_chat"]"#, None, "openai_chat");
        let configs = legacy_protocol_configs(legacy_source(&channel));
        let mut encoded = configs;
        encoded[1].mode = ProtocolMode::Convert;
        encoded[1].target = Some("anthropic".to_string());
        let mut with_configs = config_channel("[]", None, "openai_chat");
        with_configs.protocol_configs = serde_json::to_string(&encoded).expect("json");

        // `anthropic` is itself a convert route, so it cannot be a target.
        assert!(resolve_route(&with_configs, "openai_responses").is_none());
    }

    #[test]
    fn catalog_models_unions_native_configs() {
        let channel = config_channel(r#"["openai_chat"]"#, None, "openai_chat");
        let mut configs = legacy_protocol_configs(legacy_source(&channel));
        configs[2].mode = ProtocolMode::Native;
        configs[2].models = vec!["claude-x".to_string()];
        let mut channel = config_channel("[]", None, "openai_chat");
        channel.protocol_configs = serde_json::to_string(&configs).expect("json");

        let catalog = catalog_models(&channel);
        assert_eq!(catalog.models, vec!["claude-x", "m1"]);
        assert!(!catalog.has_wildcard);
    }

    #[tokio::test]
    async fn backfill_generates_protocol_routes_for_legacy_rows() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory database");
        sqlx::query(
            "CREATE TABLE channels (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                kinds TEXT NOT NULL,
                fallback_kind TEXT,
                protocol_configs TEXT NOT NULL DEFAULT '[]',
                base_url TEXT NOT NULL,
                api_key_enc TEXT NOT NULL,
                auth_header TEXT NOT NULL,
                models TEXT NOT NULL,
                model_mapping TEXT NOT NULL,
                extra_headers TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("schema");
        sqlx::query(
            "INSERT INTO channels
             (id, kind, kinds, fallback_kind, base_url, api_key_enc, auth_header, models, model_mapping, extra_headers)
             VALUES (1, 'openai_chat', '[\"openai_chat\",\"anthropic\"]', 'openai_chat',
                     'http://old.example/v1', 'encrypted', 'bearer', '[\"m1\"]', '{}', '{}')",
        )
        .execute(&pool)
        .await
        .expect("seed");

        backfill_protocol_configs(&pool).await.expect("backfill");

        let encoded: String =
            sqlx::query_scalar("SELECT protocol_configs FROM channels WHERE id = 1")
                .fetch_one(&pool)
                .await
                .expect("row");
        let configs: Vec<ProtocolConfig> = serde_json::from_str(&encoded).expect("json");
        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0].mode, ProtocolMode::Native);
        assert_eq!(configs[1].mode, ProtocolMode::Convert);
        assert_eq!(configs[1].target.as_deref(), Some("openai_chat"));
        assert_eq!(configs[1].base_url, "http://old.example/v1");
        assert_eq!(configs[2].mode, ProtocolMode::Native);

        // Running again must not rewrite already migrated rows.
        backfill_protocol_configs(&pool).await.expect("idempotent");
        let after: String =
            sqlx::query_scalar("SELECT protocol_configs FROM channels WHERE id = 1")
                .fetch_one(&pool)
                .await
                .expect("row");
        assert_eq!(after, encoded);
    }
}
