use axum::{
    extract::{FromRequestParts, State},
    http::{header, request::Parts, StatusCode},
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    error::AppError,
    models::{self, admin::Admin},
    security,
    state::AppState,
};

pub const SESSION_COOKIE: &str = "oneapi_session";
const SESSION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub struct AdminSession {
    pub admin: Admin,
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
    Json(input): Json<LoginInput>,
) -> Result<(CookieJar, Json<AdminOutput>), AppError> {
    let username = input.username.trim();
    if username.is_empty() || input.password.is_empty() {
        return Err(AppError::BadRequest(
            "username and password are required".to_string(),
        ));
    }

    let admin = models::admin::find_by_username(&state.db, username)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if !security::verify_password(&admin.password_hash, &input.password) {
        return Err(AppError::Unauthorized);
    }

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

async fn issue_session(
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

    let username = std::env::var("ONEAPI_ADMIN_USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "admin".to_string());
    let provided = std::env::var("ONEAPI_ADMIN_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());
    let generated = provided.is_none();
    let password = provided.unwrap_or_else(|| security::random_password(20));

    if password.chars().count() < 8 {
        return Err(anyhow::anyhow!(
            "ONEAPI_ADMIN_PASSWORD must be at least 8 characters"
        ));
    }

    let password_hash = security::hash_password(&password)?;
    models::admin::create(pool, &username, &password_hash, models::now_ms()).await?;

    if generated {
        tracing::warn!(
            username = %username,
            password = %password,
            "generated initial admin password; change it after first login"
        );
    } else {
        tracing::info!(username = %username, "initial admin account created");
    }

    Ok(())
}
