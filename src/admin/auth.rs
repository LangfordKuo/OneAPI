use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, State},
    http::{request::Parts, StatusCode},
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use zeroize::Zeroizing;

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
        // An explicitly configured header is trusted (the administrator opted
        // in for a CDN/proxy). An empty setting defaults to X-Forwarded-For
        // only for loopback/private peers, so a direct public client cannot
        // spoof it.
        let configured = match state.cache.client_ip_header(&state.db).await {
            Ok(configured) => configured,
            Err(error) => {
                // We cannot tell whether a header is in effect. An empty
                // setting would fall back to the TCP peer, which behind a CDN
                // is the shared origin address; report an unknown client
                // instead so limiting keeps applying to an unattributable peer
                // without ever blaming that address.
                tracing::warn!(error = %error, "failed to load client_ip_header setting");
                return Ok(Self(None));
            }
        };

        Ok(Self(client_ip::resolve_client_ip(
            &parts.headers,
            &configured,
            peer,
        )))
    }
}

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Parse the header the same way the login/logout handlers write it,
        // instead of hand-rolling a second cookie parser.
        let token = CookieJar::from_headers(&parts.headers)
            .get(SESSION_COOKIE)
            .map(|cookie| cookie.value().to_string())
            .ok_or(AppError::Unauthorized)?;

        let token_hash = security::sha256_hex(&token);
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
    let LoginInput { username, password } = input;
    let username = username.trim();
    // The plaintext password is wiped as soon as this handler returns.
    let password = Zeroizing::new(password);
    if username.is_empty() || password.is_empty() {
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
        // Keep the Argon2 cost identical to the wrong-password path so response
        // timing does not reveal whether the username exists.
        security::verify_password(security::dummy_password_hash(), password.as_str());
        state.login_limiter.record_failure(&limiter_key);
        return Err(AppError::Unauthorized);
    };

    if !security::verify_password(&admin.password_hash, password.as_str()) {
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
    client_addr: ClientAddr,
    Json(input): Json<ChangePasswordInput>,
) -> Result<(CookieJar, Json<AdminOutput>), AppError> {
    let ChangePasswordInput {
        current_password,
        new_password,
    } = input;
    // Wipe both plaintext passwords when this handler returns.
    let current_password = Zeroizing::new(current_password);
    let new_password = Zeroizing::new(new_password);

    let client_ip = client_addr
        .0
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let limiter_key = format!("password|{client_ip}|{}", session.admin.id);
    if let Some(wait) = state.login_limiter.blocked_for(&limiter_key) {
        return Err(AppError::TooManyRequests(format!(
            "too many failed password attempts; retry in {} seconds",
            wait.as_secs().max(1)
        )));
    }

    if !security::verify_password(&session.admin.password_hash, current_password.as_str()) {
        state.login_limiter.record_failure(&limiter_key);
        return Err(AppError::Unauthorized);
    }
    state.login_limiter.record_success(&limiter_key);

    if let Some(violation) = security::password_violation(new_password.as_str()) {
        return Err(AppError::BadRequest(format!("new password {violation}")));
    }

    let password_hash =
        security::hash_password(new_password.as_str()).map_err(AppError::Internal)?;
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
    // The bootstrap password comes from the environment as plaintext.
    let password = Zeroizing::new(password);

    let username = std::env::var("ONEAPI_ADMIN_USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "admin".to_string());

    if let Some(violation) = security::password_violation(password.as_str()) {
        return Err(anyhow::anyhow!("ONEAPI_ADMIN_PASSWORD {violation}"));
    }

    let password_hash = security::hash_password(password.as_str())?;
    models::admin::create(pool, &username, &password_hash, models::now_ms()).await?;
    tracing::info!(username = %username, "initial admin account created from environment");

    Ok(())
}
