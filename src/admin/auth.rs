use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, State},
    http::{header, request::Parts, StatusCode},
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    client_ip,
    error::AppError,
    models::{self, admin::Admin},
    security,
    state::AppState,
};

pub const SESSION_COOKIE: &str = "oneapi_session";
const SESSION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Default)]
pub struct LoginLimiter {
    attempts: Mutex<HashMap<String, Attempt>>,
}

#[derive(Debug, Clone, Copy)]
struct Attempt {
    failures: u32,
    last_failure: Instant,
    blocked_until: Option<Instant>,
}

impl LoginLimiter {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Attempt>> {
        self.attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Remaining lockout for a client/username pair, if any.
    pub fn blocked_for(&self, key: &str) -> Option<Duration> {
        let now = Instant::now();
        let attempts = self.lock();
        match attempts.get(key).and_then(|entry| entry.blocked_until) {
            Some(until) if until > now => Some(until - now),
            _ => None,
        }
    }

    pub fn record_failure(&self, key: &str) {
        let now = Instant::now();
        let mut attempts = self.lock();
        if attempts.len() > 10_000 {
            attempts.retain(|_, entry| {
                now.duration_since(entry.last_failure) < Duration::from_secs(3_600)
            });
        }

        let entry = attempts.entry(key.to_string()).or_insert(Attempt {
            failures: 0,
            last_failure: now,
            blocked_until: None,
        });
        entry.failures = entry.failures.saturating_add(1);
        entry.last_failure = now;

        let block_secs = match entry.failures {
            5..=9 => 30,
            10..=14 => 300,
            15.. => 900,
            _ => 0,
        };
        if block_secs > 0 {
            entry.blocked_until = Some(now + Duration::from_secs(block_secs));
        }
    }

    pub fn record_success(&self, key: &str) {
        self.lock().remove(key);
    }
}

pub struct AdminSession {
    pub admin: Admin,
}

/// Optional client address. `ConnectInfo` is only present when the server is
/// started with connect-info support (and absent in oneshot tests).
pub struct ClientAddr(pub Option<std::net::IpAddr>);

impl FromRequestParts<AppState> for ClientAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        let header = models::setting::get(&state.db, client_ip::SETTING_KEY)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();

        Ok(Self(client_ip::resolve(&parts.headers, &header, peer)))
    }
}

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').find_map(|cookie| {
                    let (name, value) = cookie.trim().split_once('=')?;
                    (name == SESSION_COOKIE).then_some(value)
                })
            })
            .ok_or(AppError::Unauthorized)?;

        let token_hash = security::sha256_hex(token);
        let admin = models::session::find_admin(&state.db, &token_hash, models::now_ms())
            .await?
            .ok_or(AppError::Unauthorized)?;

        Ok(Self { admin })
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AdminOutput {
    pub username: String,
    pub created_at: i64,
}

impl From<&Admin> for AdminOutput {
    fn from(admin: &Admin) -> Self {
        Self {
            username: admin.username.clone(),
            created_at: admin.created_at,
        }
    }
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    client_addr: ClientAddr,
    Json(input): Json<LoginInput>,
) -> Result<(CookieJar, Json<AdminOutput>), AppError> {
    let username = input.username.trim();
    if username.is_empty() || input.password.is_empty() {
        return Err(AppError::BadRequest(
            "username and password are required".to_string(),
        ));
    }

    let client_ip = client_addr
        .0
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let limiter_key = format!("{client_ip}|{username}");

    if let Some(wait) = state.login_limiter.blocked_for(&limiter_key) {
        return Err(AppError::TooManyRequests(format!(
            "too many failed login attempts; retry in {} seconds",
            wait.as_secs().max(1)
        )));
    }

    let Some(admin) = models::admin::find_by_username(&state.db, username).await? else {
        state.login_limiter.record_failure(&limiter_key);
        return Err(AppError::Unauthorized);
    };

    if !security::verify_password(&admin.password_hash, &input.password) {
        state.login_limiter.record_failure(&limiter_key);
        return Err(AppError::Unauthorized);
    }

    state.login_limiter.record_success(&limiter_key);
    let jar = issue_session(&state, jar, admin.id).await?;
    Ok((jar, Json(AdminOutput::from(&admin))))
}

pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), AppError> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let token_hash = security::sha256_hex(cookie.value());
        models::session::delete(&state.db, &token_hash).await?;
    }

    let jar = jar.remove(Cookie::build((SESSION_COOKIE, "")).path("/"));
    Ok((jar, StatusCode::NO_CONTENT))
}

pub async fn me(session: AdminSession) -> Json<AdminOutput> {
    Json(AdminOutput::from(&session.admin))
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordInput {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    session: AdminSession,
    jar: CookieJar,
    Json(input): Json<ChangePasswordInput>,
) -> Result<(CookieJar, Json<AdminOutput>), AppError> {
    if !security::verify_password(&session.admin.password_hash, &input.current_password) {
        return Err(AppError::Unauthorized);
    }

    if input.new_password.chars().count() < 8 {
        return Err(AppError::BadRequest(
            "new password must be at least 8 characters".to_string(),
        ));
    }

    let password_hash = security::hash_password(&input.new_password).map_err(AppError::Internal)?;
    models::admin::update_password(&state.db, session.admin.id, &password_hash).await?;
    models::session::delete_all_for_admin(&state.db, session.admin.id).await?;

    let jar = issue_session(&state, jar, session.admin.id).await?;
    Ok((jar, Json(AdminOutput::from(&session.admin))))
}

pub(crate) async fn issue_session(
    state: &AppState,
    jar: CookieJar,
    admin_id: i64,
) -> Result<CookieJar, AppError> {
    let token = security::generate_session_token();
    let token_hash = security::sha256_hex(&token);
    let now = models::now_ms();

    models::session::delete_expired(&state.db, now).await?;
    models::session::create(&state.db, &token_hash, admin_id, now, now + SESSION_TTL_MS).await?;

    let cookie = Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.config.cookie_secure)
        .max_age(time::Duration::seconds(SESSION_TTL_MS / 1000));

    Ok(jar.add(cookie))
}

pub async fn bootstrap(pool: &SqlitePool) -> anyhow::Result<()> {
    if models::admin::count(pool).await? > 0 {
        return Ok(());
    }

    let provided = std::env::var("ONEAPI_ADMIN_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());
    let Some(password) = provided else {
        tracing::info!("OneAPI is not initialized; open the web UI to create the admin account");
        return Ok(());
    };

    let username = std::env::var("ONEAPI_ADMIN_USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "admin".to_string());

    if password.chars().count() < 8 {
        return Err(anyhow::anyhow!(
            "ONEAPI_ADMIN_PASSWORD must be at least 8 characters"
        ));
    }

    let password_hash = security::hash_password(&password)?;
    models::admin::create(pool, &username, &password_hash, models::now_ms()).await?;
    tracing::info!(username = %username, "initial admin account created from environment");

    Ok(())
}
