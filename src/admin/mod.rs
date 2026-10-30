mod auth;
mod backup;
mod catalog;
mod channels;
mod csrf;
mod dashboard;
mod keys;
mod logs;
mod metrics;
mod playground;
mod query;
mod settings;
mod setup;
mod update;

pub use auth::{bootstrap, AdminSession, LoginLimiter};

use axum::{
    middleware,
    routing::{get, post, put},
    Router,
};

use crate::state::AppState;

pub fn routes(state: AppState) -> Router<AppState> {
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
        .route("/api/channels/test", post(channels::probe_test))
        .route(
            "/api/channels/fetch-models",
            post(channels::probe_fetch_models),
        )
        .route("/api/channels/{id}/test", post(channels::test))
        .route("/api/channels/{id}/enabled", put(channels::set_enabled))
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
        .route("/api/metrics/stream", get(metrics::stream))
        .route("/api/update/check", get(update::check))
        .route("/api/update/apply", post(update::apply))
        .route("/api/backup/snapshot", post(backup::snapshot))
        .route("/api/backup/list", get(backup::list))
        .layer(middleware::from_fn_with_state(state, csrf::guard))
}
