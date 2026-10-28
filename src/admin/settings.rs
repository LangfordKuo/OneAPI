use std::collections::HashMap;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::{client_ip, error::AppError, models::setting, state::AppState};

use super::auth::AdminSession;

#[derive(Debug, Serialize)]
pub struct SettingsOutput {
    pub site_name: String,
    pub site_domain: String,
    pub default_max_tokens: i64,
    pub nonstream_timeout_seconds: i64,
    pub stream_idle_timeout_seconds: i64,
    pub usage_log_retention_days: i64,
    pub response_retention_days: i64,
    pub store_request_bodies: bool,
    pub update_source: String,
    pub client_ip_header: String,
}

impl SettingsOutput {
    fn from_rows(rows: Vec<(String, String)>) -> Self {
        let map: HashMap<String, String> = rows.into_iter().collect();
        let integer = |key: &str, default: i64| {
            map.get(key)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(default)
        };
        let boolean = |key: &str, default: bool| {
            map.get(key)
                .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
                .unwrap_or(default)
        };

        Self {
            site_name: map
                .get("site_name")
                .cloned()
                .unwrap_or_else(|| "OneAPI".to_string()),
            site_domain: map
                .get("site_domain")
                .and_then(|value| normalize_authority(value))
                .unwrap_or_default(),
            default_max_tokens: integer("default_max_tokens", 4096),
            nonstream_timeout_seconds: integer("nonstream_timeout_seconds", 600),
            stream_idle_timeout_seconds: integer("stream_idle_timeout_seconds", 120),
            usage_log_retention_days: integer("usage_log_retention_days", 30),
            response_retention_days: integer("response_retention_days", 7),
            store_request_bodies: boolean("store_request_bodies", false),
            update_source: map
                .get("update_source")
                .filter(|value| value.as_str() == "github" || value.as_str() == "ghproxy")
                .cloned()
                .unwrap_or_else(|| "github".to_string()),
            client_ip_header: map.get(client_ip::SETTING_KEY).cloned().unwrap_or_default(),
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
    pub response_retention_days: Option<i64>,
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
    if let Some(site_name) = input.site_name {
        let site_name = site_name.trim();
        if site_name.is_empty() {
            return Err(AppError::BadRequest(
                "site_name cannot be empty".to_string(),
            ));
        }
        setting::upsert(&state.db, "site_name", site_name).await?;
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
        setting::upsert(&state.db, "site_domain", &normalized).await?;
    }

    if let Some(value) = input.default_max_tokens {
        if value <= 0 {
            return Err(AppError::BadRequest(
                "default_max_tokens must be positive".to_string(),
            ));
        }
        setting::upsert(&state.db, "default_max_tokens", &value.to_string()).await?;
    }

    if let Some(value) = input.nonstream_timeout_seconds {
        if value <= 0 {
            return Err(AppError::BadRequest(
                "nonstream_timeout_seconds must be positive".to_string(),
            ));
        }
        setting::upsert(&state.db, "nonstream_timeout_seconds", &value.to_string()).await?;
    }

    if let Some(value) = input.stream_idle_timeout_seconds {
        if value <= 0 {
            return Err(AppError::BadRequest(
                "stream_idle_timeout_seconds must be positive".to_string(),
            ));
        }
        setting::upsert(&state.db, "stream_idle_timeout_seconds", &value.to_string()).await?;
    }

    if let Some(value) = input.usage_log_retention_days {
        if value < 0 {
            return Err(AppError::BadRequest(
                "usage_log_retention_days cannot be negative".to_string(),
            ));
        }
        setting::upsert(&state.db, "usage_log_retention_days", &value.to_string()).await?;
    }

    if let Some(value) = input.response_retention_days {
        if value < 0 {
            return Err(AppError::BadRequest(
                "response_retention_days cannot be negative".to_string(),
            ));
        }
        setting::upsert(&state.db, "response_retention_days", &value.to_string()).await?;
    }

    if let Some(value) = input.store_request_bodies {
        setting::upsert(
            &state.db,
            "store_request_bodies",
            if value { "true" } else { "false" },
        )
        .await?;
    }

    if let Some(value) = input.update_source {
        let value = value.trim();
        if !matches!(value, "github" | "ghproxy") {
            return Err(AppError::BadRequest(
                "update_source must be github or ghproxy".to_string(),
            ));
        }
        setting::upsert(&state.db, "update_source", value).await?;
    }

    if let Some(value) = input.client_ip_header {
        let Some(header) = client_ip::parse_header_name(&value) else {
            return Err(AppError::BadRequest(
                "client_ip_header is not a valid HTTP header name".to_string(),
            ));
        };
        setting::upsert(&state.db, client_ip::SETTING_KEY, &header).await?;
    }

    state.cache.invalidate_defaults();
    state.cache.invalidate_site_domain();
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

#[cfg(test)]
mod tests {
    use super::normalize_authority;

    #[test]
    fn normalizes_hosts_and_urls_to_authority() {
        assert_eq!(
            normalize_authority("example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_authority("https://Example.COM/path?q=1").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_authority("https://example.com:443").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_authority("http://example.com:8080").as_deref(),
            Some("example.com:8080")
        );
        assert_eq!(
            normalize_authority("[::1]:8443").as_deref(),
            Some("[::1]:8443")
        );
        assert_eq!(normalize_authority("[::1]:443").as_deref(), Some("[::1]"));
        assert_eq!(
            normalize_authority("example.com.").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn rejects_invalid_authorities() {
        assert_eq!(normalize_authority(""), None);
        assert_eq!(normalize_authority("https://"), None);
        assert_eq!(normalize_authority("user@example.com"), None);
        assert_eq!(normalize_authority("example.com:0"), None);
        assert_eq!(normalize_authority("example.com:not-a-port"), None);
        assert_eq!(normalize_authority("[::1"), None);
        assert_eq!(normalize_authority("not a domain"), None);
        assert_eq!(normalize_authority("https://exa mple.com"), None);
    }
}
