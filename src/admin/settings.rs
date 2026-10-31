use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::{client_ip, error::AppError, models::setting, state::AppState};

use super::auth::AdminSession;

/// Upper bounds for numeric gateway settings. Without them a typo such as
/// `9999999999999999` is accepted, and `GatewayDefaults::load` later either
/// overflows when converting seconds to milliseconds or silently falls back to
/// the built-in default.
const MAX_DEFAULT_MAX_TOKENS: i64 = 1_000_000;
const MAX_TIMEOUT_SECONDS: i64 = 86_400;
/// Upper bound for retention windows. `0` disables time-based cleanup for the
/// category instead of meaning "delete immediately".
const MAX_RETENTION_DAYS: i64 = 3_650;
/// Upper bound for Responses storage quotas, in MiB (1 TiB). `0` means
/// unlimited.
const MAX_RESPONSE_STORAGE_MB: i64 = 1_048_576;

fn check_range(field: &str, value: i64, min: i64, max: i64) -> Result<(), AppError> {
    if !(min..=max).contains(&value) {
        return Err(AppError::BadRequest(format!(
            "{field} must be between {min} and {max}"
        )));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SettingsOutput {
    pub site_name: String,
    pub site_domain: String,
    pub default_max_tokens: i64,
    pub nonstream_timeout_seconds: i64,
    pub stream_idle_timeout_seconds: i64,
    pub usage_log_retention_days: i64,
    pub request_detail_retention_days: i64,
    pub response_retention_days: i64,
    pub response_storage_per_key_mb: i64,
    pub response_storage_total_mb: i64,
    pub store_request_bodies: bool,
    pub update_source: String,
    pub client_ip_header: String,
}

impl SettingsOutput {
    fn from_rows(rows: Vec<(String, String)>) -> Self {
        let map = setting::SettingsMap::new(rows);

        Self {
            site_name: map.string("site_name").unwrap_or("OneAPI").to_string(),
            site_domain: map
                .string("site_domain")
                .and_then(normalize_authority)
                .unwrap_or_default(),
            default_max_tokens: map.integer("default_max_tokens", 4096),
            nonstream_timeout_seconds: map.integer("nonstream_timeout_seconds", 600),
            stream_idle_timeout_seconds: map.integer("stream_idle_timeout_seconds", 120),
            usage_log_retention_days: map.integer("usage_log_retention_days", 30),
            request_detail_retention_days: map.integer("request_detail_retention_days", 7),
            response_retention_days: map.integer("response_retention_days", 7),
            response_storage_per_key_mb: map.integer(
                "response_storage_per_key_mb",
                setting::DEFAULT_RESPONSE_STORAGE_PER_KEY_MB,
            ),
            response_storage_total_mb: map.integer(
                "response_storage_total_mb",
                setting::DEFAULT_RESPONSE_STORAGE_TOTAL_MB,
            ),
            store_request_bodies: map.boolean("store_request_bodies", false),
            update_source: map
                .string("update_source")
                .filter(|value| *value == "github" || *value == "ghproxy")
                .unwrap_or("github")
                .to_string(),
            client_ip_header: map
                .string(client_ip::SETTING_KEY)
                .unwrap_or_default()
                .to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SettingsInput {
    #[serde(default)]
    pub site_name: Option<String>,
    #[serde(default)]
    pub site_domain: Option<String>,
    #[serde(default)]
    pub default_max_tokens: Option<i64>,
    #[serde(default)]
    pub nonstream_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub stream_idle_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub usage_log_retention_days: Option<i64>,
    #[serde(default)]
    pub request_detail_retention_days: Option<i64>,
    #[serde(default)]
    pub response_retention_days: Option<i64>,
    #[serde(default)]
    pub response_storage_per_key_mb: Option<i64>,
    #[serde(default)]
    pub response_storage_total_mb: Option<i64>,
    #[serde(default)]
    pub store_request_bodies: Option<bool>,
    #[serde(default)]
    pub update_source: Option<String>,
    #[serde(default)]
    pub client_ip_header: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<SettingsOutput>, AppError> {
    let rows = setting::all(&state.db).await?;
    Ok(Json(SettingsOutput::from_rows(rows)))
}

pub async fn update(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<SettingsInput>,
) -> Result<Json<SettingsOutput>, AppError> {
    // Validate and collect everything first so a later validation failure does
    // not leave an earlier field persisted.
    let mut updates: Vec<(&'static str, String)> = Vec::new();

    if let Some(site_name) = input.site_name {
        let site_name = site_name.trim();
        if site_name.is_empty() {
            return Err(AppError::BadRequest(
                "site_name cannot be empty".to_string(),
            ));
        }
        updates.push(("site_name", site_name.to_string()));
    }

    if let Some(value) = input.site_domain {
        let value = value.trim();
        let normalized = if value.is_empty() {
            String::new()
        } else {
            normalize_authority(value).ok_or_else(|| {
                AppError::BadRequest(
                    "site_domain must be a host name or an http(s) URL".to_string(),
                )
            })?
        };
        updates.push(("site_domain", normalized));
    }

    if let Some(value) = input.default_max_tokens {
        check_range("default_max_tokens", value, 1, MAX_DEFAULT_MAX_TOKENS)?;
        updates.push(("default_max_tokens", value.to_string()));
    }

    if let Some(value) = input.nonstream_timeout_seconds {
        check_range("nonstream_timeout_seconds", value, 1, MAX_TIMEOUT_SECONDS)?;
        updates.push(("nonstream_timeout_seconds", value.to_string()));
    }

    if let Some(value) = input.stream_idle_timeout_seconds {
        check_range("stream_idle_timeout_seconds", value, 1, MAX_TIMEOUT_SECONDS)?;
        updates.push(("stream_idle_timeout_seconds", value.to_string()));
    }

    if let Some(value) = input.usage_log_retention_days {
        if value < 0 {
            return Err(AppError::BadRequest(
                "usage_log_retention_days cannot be negative".to_string(),
            ));
        }
        updates.push(("usage_log_retention_days", value.to_string()));
    }

    if let Some(value) = input.request_detail_retention_days {
        check_range(
            "request_detail_retention_days",
            value,
            0,
            MAX_RETENTION_DAYS,
        )?;
        updates.push(("request_detail_retention_days", value.to_string()));
    }

    if let Some(value) = input.response_retention_days {
        if value < 0 {
            return Err(AppError::BadRequest(
                "response_retention_days cannot be negative".to_string(),
            ));
        }
        updates.push(("response_retention_days", value.to_string()));
    }

    if let Some(value) = input.response_storage_per_key_mb {
        check_range(
            "response_storage_per_key_mb",
            value,
            0,
            MAX_RESPONSE_STORAGE_MB,
        )?;
        updates.push(("response_storage_per_key_mb", value.to_string()));
    }

    if let Some(value) = input.response_storage_total_mb {
        check_range(
            "response_storage_total_mb",
            value,
            0,
            MAX_RESPONSE_STORAGE_MB,
        )?;
        updates.push(("response_storage_total_mb", value.to_string()));
    }

    if let Some(value) = input.store_request_bodies {
        updates.push((
            "store_request_bodies",
            if value { "true" } else { "false" }.to_string(),
        ));
    }

    if let Some(value) = input.update_source {
        let value = value.trim();
        if !matches!(value, "github" | "ghproxy") {
            return Err(AppError::BadRequest(
                "update_source must be github or ghproxy".to_string(),
            ));
        }
        updates.push(("update_source", value.to_string()));
    }

    if let Some(value) = input.client_ip_header {
        let Some(header) = client_ip::parse_header_name(&value) else {
            return Err(AppError::BadRequest(
                "client_ip_header is not a valid HTTP header name".to_string(),
            ));
        };
        updates.push((client_ip::SETTING_KEY, header));
    }

    let mut tx = state.db.begin().await?;
    for (key, value) in &updates {
        setting::upsert(&mut *tx, key, value).await?;
    }
    tx.commit().await?;

    state.cache.invalidate_defaults();
    state.cache.invalidate_site_domain();
    state.cache.invalidate_site_name();
    state.cache.invalidate_client_ip_header();
    state.clients.clear();

    let rows = setting::all(&state.db).await?;
    Ok(Json(SettingsOutput::from_rows(rows)))
}

/// Normalize a Host header, Origin header or configured domain to `host[:port]`.
/// Scheme, path, query and fragment are ignored on purpose: TLS usually ends at
/// the reverse proxy, so the browser's scheme and the backend's view differ.
/// Default ports (80/443) are dropped so `example.com:443` matches
/// `https://example.com`.
pub(crate) fn normalize_authority(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let authority = raw.split_once("://").map(|(_, rest)| rest).unwrap_or(raw);
    let authority = authority.split(['/', '?', '#']).next()?.trim();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }

    let authority = authority.to_ascii_lowercase();
    let authority = authority.trim_end_matches('.');

    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':');
        (format!("[{host}]"), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !port.is_empty() => (host.to_string(), Some(port)),
            _ => (authority.to_string(), None),
        }
    };

    if host.is_empty() {
        return None;
    }

    let valid_host = if let Some(inner) = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        !inner.is_empty()
            && inner.chars().all(|character| {
                character.is_ascii_hexdigit() || character == ':' || character == '.'
            })
    } else {
        host.len() <= 253
            && host.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_')
            })
    };
    if !valid_host {
        return None;
    }

    let port = match port {
        Some(value) => {
            let number = value.parse::<u16>().ok().filter(|value| *value != 0)?;
            if number == 80 || number == 443 {
                None
            } else {
                Some(number)
            }
        }
        None => None,
    };

    Some(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}
