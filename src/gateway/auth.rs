use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    error::AppError,
    models::{
        self,
        api_key::{ApiKey, ApiKeyLimits},
    },
    security,
    state::AppState,
};

use super::error::{client_error, protocol_for_path};

pub struct ApiKeyAuth {
    pub key: ApiKey,
    pub limits: ApiKeyLimits,
}

impl FromRequestParts<AppState> for ApiKeyAuth {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Limit by the real TCP peer; forwarded headers are client controlled
        // and must not be able to shape another client's window.
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());

        // Fail before touching the database: a peer that keeps sending bad
        // keys must not get a lookup per attempt.
        if !state.auth_failure_limiter.check(peer) {
            tracing::warn!(peer = ?peer, "API key authentication rate limit exceeded");
            return Err(rate_limited(parts));
        }

        let raw_key = match extract_api_key(parts) {
            Some(key) => key,
            None => {
                state.auth_failure_limiter.record_failure(peer);
                return Err(unauthorized(parts));
            }
        };

        let v2_hash = state.crypto.api_key_hash_v2(&raw_key);
        let legacy_hash = security::Crypto::api_key_hash_legacy(&raw_key);

        let record = models::api_key::find_by_key_hash(&state.db, &v2_hash, &legacy_hash)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "failed to look up API key");
                server_error(parts)
            })?
            .ok_or_else(|| {
                state.auth_failure_limiter.record_failure(peer);
                tracing::debug!(
                    hash_prefix = %legacy_hash.get(..8).unwrap_or(legacy_hash.as_str()),
                    "API key authentication failed"
                );
                unauthorized(parts)
            })?;

        let now = models::now_ms();

        // The credentials matched a stored hash, so clear this peer's failure
        // history even if a quota check below rejects the request.
        state.auth_failure_limiter.record_success(peer);

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
            state.auth_failure_limiter.record_failure(peer);
            return Err(unauthorized(parts));
        }

        // Rows created before migration 0009 only carry the legacy hash. Fill
        // in the peppered one on first successful use; a failed backfill must
        // never break authentication.
        if record.key_hash_v2.is_none() {
            if let Err(error) =
                models::api_key::set_key_hash_v2(&state.db, record.id, &v2_hash).await
            {
                tracing::warn!(
                    error = %error,
                    key_id = record.id,
                    "failed to backfill API key hash"
                );
            }
        }

        // Parse the restrictions once and fail closed: a malformed allowlist
        // must not be treated as an empty (unrestricted) list, so the request
        // never reaches channel selection.
        let limits = match record.parse_limits() {
            Ok(limits) => limits,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    key_id = record.id,
                    "failed to parse API key restrictions"
                );
                return Err(AppError::Internal(anyhow::anyhow!(
                    "failed to parse restrictions for API key {}: {error}",
                    record.id
                ))
                .into_response());
            }
        };

        if record
            .last_used_at
            .map(|last_used| now - last_used > 60_000)
            .unwrap_or(true)
        {
            if let Err(error) = models::api_key::touch_last_used(&state.db, record.id, now).await {
                tracing::warn!(error = %error, key_id = record.id, "failed to update API key usage");
            }
        }

        Ok(Self { key: record, limits })
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

fn rate_limited(parts: &Parts) -> Response {
    client_error(
        protocol_for_path(parts.uri.path()),
        StatusCode::TOO_MANY_REQUESTS,
        "too many failed authentication attempts; retry later",
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
