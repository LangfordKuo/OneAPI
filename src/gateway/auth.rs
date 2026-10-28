use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
    response::Response,
};

use crate::{
    models::{self, api_key::ApiKey},
    security,
    state::AppState,
};

use super::error::{client_error, protocol_for_path};

pub struct ApiKeyAuth {
    pub key: ApiKey,
}

impl FromRequestParts<AppState> for ApiKeyAuth {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw_key = extract_api_key(parts).ok_or_else(|| unauthorized(parts))?;
        let key_hash = security::api_key_hash(&raw_key);

        let record = models::api_key::find_by_hash(&state.db, &key_hash)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "failed to look up API key");
                server_error(parts)
            })?
            .ok_or_else(|| unauthorized(parts))?;

        let now = models::now_ms();
        if record.token_limit_reached() {
            if record.enabled != 0 {
                if let Err(error) = models::api_key::disable(&state.db, record.id).await {
                    tracing::warn!(error = %error, key_id = record.id, "failed to disable over-quota API key");
                }
            }
            return Err(limit_reached(parts));
        }

        if record.daily_limit_reached(now / 86_400_000) {
            return Err(daily_limit_reached(parts));
        }

        if record.enabled == 0
            || record
                .expires_at
                .map(|expires_at| expires_at <= now)
                .unwrap_or(false)
        {
            return Err(unauthorized(parts));
        }

        if record
            .last_used_at
            .map(|last_used| now - last_used > 60_000)
            .unwrap_or(true)
        {
            if let Err(error) = models::api_key::touch_last_used(&state.db, record.id, now).await {
                tracing::warn!(error = %error, key_id = record.id, "failed to update API key usage");
            }
        }

        Ok(Self { key: record })
    }
}

fn extract_api_key(parts: &Parts) -> Option<String> {
    if let Some(value) = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    {
        if let Some((scheme, token)) = value.split_once(' ') {
            if scheme.eq_ignore_ascii_case("bearer") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    parts
        .headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn unauthorized(parts: &Parts) -> Response {
    client_error(
        protocol_for_path(parts.uri.path()),
        StatusCode::UNAUTHORIZED,
        "invalid or missing API key",
        "authentication_error",
    )
}

fn limit_reached(parts: &Parts) -> Response {
    client_error(
        protocol_for_path(parts.uri.path()),
        StatusCode::FORBIDDEN,
        "API key token limit reached",
        "permission_error",
    )
}

fn daily_limit_reached(parts: &Parts) -> Response {
    client_error(
        protocol_for_path(parts.uri.path()),
        StatusCode::TOO_MANY_REQUESTS,
        "API key daily token limit reached; it resets at 00:00 UTC",
        "rate_limit_error",
    )
}

fn server_error(parts: &Parts) -> Response {
    client_error(
        protocol_for_path(parts.uri.path()),
        StatusCode::INTERNAL_SERVER_ERROR,
        "authentication service unavailable",
        "api_error",
    )
}
