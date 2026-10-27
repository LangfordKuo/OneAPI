mod auth;
mod catalog;
mod channels;
mod csrf;
mod dashboard;
mod keys;
mod logs;
mod playground;
mod query;
mod settings;
mod setup;

#[cfg(test)]
mod tests;

pub use auth::bootstrap;

use axum::{
    middleware,
    routing::{get, post, put},
    Router,
};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/password", put(auth::change_password))
        .route("/api/channels", get(channels::list).post(channels::create))
        .route(
            "/api/channels/{id}",
            get(channels::get)
                .put(channels::update)
                .delete(channels::delete),
        )
        .route("/api/channels/{id}/test", post(channels::test))
        .route(
            "/api/channels/{id}/fetch-models",
            post(channels::fetch_models),
        )
        .route("/api/keys", get(keys::list).post(keys::create))
        .route("/api/keys/{id}", put(keys::update).delete(keys::delete))
        .route("/api/settings", get(settings::get).put(settings::update))
        .route("/api/logs", get(logs::list))
        .route("/api/logs/{request_id}", get(logs::detail))
        .route("/api/dashboard/summary", get(dashboard::summary))
        .route("/api/dashboard/timeseries", get(dashboard::timeseries))
        .route("/api/dashboard/breakdown", get(dashboard::breakdown))
        .route("/api/models", get(catalog::list))
        .route("/api/playground", post(playground::proxy))
        .route("/api/setup/status", get(setup::status))
        .route("/api/setup", post(setup::setup))
        .layer(middleware::from_fn(csrf::guard))
}
