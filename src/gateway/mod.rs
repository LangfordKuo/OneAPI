use std::collections::BTreeSet;

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

use crate::{error::AppError, models, protocol::Protocol, state::AppState};

use auth::ApiKeyAuth;

pub mod auth;
pub mod error;
pub mod execute;
pub mod select;
pub mod usage;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/responses", post(responses))
        .route("/v1/messages", post(messages))
        .route("/v1/models", get(list_models))
        .route("/v1/models/{id}", get(get_model))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(CorsLayer::permissive())
}

async fn chat(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AppError> {
    execute::proxy(&state, &auth, Protocol::Chat, &headers, body).await
}

async fn responses(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AppError> {
    execute::proxy(&state, &auth, Protocol::Responses, &headers, body).await
}

async fn messages(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AppError> {
    execute::proxy(&state, &auth, Protocol::Anthropic, &headers, body).await
}

async fn list_models(
    State(state): State<AppState>,
    _auth: ApiKeyAuth,
) -> Result<Json<Value>, AppError> {
    let models = available_models(&state.db).await?;
    let first_id = models.first().cloned();
    let last_id = models.last().cloned();
    let data: Vec<Value> = models
        .into_iter()
        .map(|id| {
            json!({
                "id": id,
                "object": "model",
                "created": 0,
                "owned_by": "oneapi",
            })
        })
        .collect();

    Ok(Json(json!({
        "object": "list",
        "data": data,
        "has_more": false,
        "first_id": first_id,
        "last_id": last_id,
    })))
}

async fn get_model(
    State(state): State<AppState>,
    _auth: ApiKeyAuth,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let models = available_models(&state.db).await?;
    if models.iter().any(|model| model == &id) {
        Ok(Json(json!({
            "id": id,
            "object": "model",
            "created": 0,
            "owned_by": "oneapi",
        }))
        .into_response())
    } else {
        Ok(error::client_error(
            Protocol::Chat,
            StatusCode::NOT_FOUND,
            "model not found",
            "not_found",
        ))
    }
}

async fn available_models(pool: &sqlx::SqlitePool) -> sqlx::Result<Vec<String>> {
    let channels = models::channel::list(pool).await?;
    let mut result = BTreeSet::new();

    for channel in channels {
        if channel.enabled == 0 {
            continue;
        }
        let channel_models: Vec<String> = serde_json::from_str(&channel.models).unwrap_or_default();
        for model in channel_models {
            if !model.is_empty() && model != "*" {
                result.insert(model);
            }
        }
    }

    Ok(result.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    use crate::{
        app,
        config::Config,
        db,
        models::{self, channel::ChannelWrite, now_ms},
        security,
        state::AppState,
    };

    fn test_config(temp: &TempDir) -> Config {
        let data_dir = temp.path().to_path_buf();
        Config {
            addr: "127.0.0.1:0"
                .parse()
                .expect("valid loopback socket address"),
            data_dir: data_dir.clone(),
            database_url: None,
            database_path: data_dir.join("test.db"),
            log_level: "error".to_string(),
            cookie_secure: false,
        }
    }

    async fn test_state() -> (TempDir, AppState) {
        let temp = TempDir::new().expect("temp dir");
        let config = test_config(&temp);
        let db = db::connect(&config).await.expect("database");
        let crypto =
            Arc::new(security::Crypto::load_or_create(&config.data_dir).expect("master key"));
        let state = AppState {
            config: Arc::new(config),
            db,
            crypto,
            started_at: chrono::Utc::now(),
        };
        (temp, state)
    }

    async fn seed_channel(
        state: &AppState,
        kind: &str,
        base_url: &str,
        models: &[&str],
        priority: i64,
        upstream_key: &str,
    ) -> i64 {
        let write = ChannelWrite {
            name: format!("test-{kind}-{priority}"),
            kind: kind.to_string(),
            base_url: base_url.to_string(),
            api_key_enc: state
                .crypto
                .encrypt(upstream_key)
                .expect("encrypt upstream key"),
            auth_header: if kind == "anthropic" {
                "x-api-key".to_string()
            } else {
                "bearer".to_string()
            },
            models: serde_json::to_string(&models).expect("models json"),
            model_mapping: "{}".to_string(),
            extra_headers: "{}".to_string(),
            proxy_url: None,
            priority,
            weight: 1,
            timeout_ms: None,
            enabled: 1,
        };

        models::channel::create(&state.db, &write, now_ms())
            .await
            .expect("create channel")
            .id
    }

    async fn seed_key(state: &AppState) -> String {
        let key = security::generate_api_key();
        let key_hash = security::api_key_hash(&key);
        let prefix: String = key.chars().take(12).collect();
        models::api_key::create(&state.db, "test", &key_hash, &prefix, 1, None, now_ms())
            .await
            .expect("create key");
        key
    }

    async fn send(
        router: axum::Router,
        path: &str,
        key: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(key) = key {
            builder = builder.header("authorization", format!("Bearer {key}"));
        }
        let request = builder
            .body(Body::from(serde_json::to_vec(&body).expect("request json")))
            .expect("request");
        let response = router.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    async fn send_get(router: axum::Router, path: &str, key: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .header("authorization", format!("Bearer {key}"))
            .body(Body::empty())
            .expect("request");
        let response = router.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    fn chat_success_body(content: &str) -> Value {
        json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "created": 123,
            "model": "upstream-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
            },
        })
    }

    #[tokio::test]
    async fn same_protocol_is_forwarded_and_logged() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        let mut response = chat_success_body("hello");
        response["custom_field"] = json!("kept");
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;

        let channel_id = seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", server.uri()),
            &["gpt-test"],
            10,
            "upstream-key",
        )
        .await;

        let router = app::router(state.clone());
        let (status, body) = send(
            router,
            "/v1/chat/completions",
            Some(&key),
            json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["custom_field"], json!("kept"));
        assert_eq!(body["choices"][0]["message"]["content"], json!("hello"));

        let (attempts, status_code, channel, prompt, completion): (
            i64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<i64>,
        ) = sqlx::query_as(
            "SELECT attempts, status_code, channel_id, prompt_tokens, completion_tokens
             FROM usage_logs ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&state.db)
        .await
        .expect("usage row");

        assert_eq!(attempts, 1);
        assert_eq!(status_code, 200);
        assert_eq!(channel, Some(channel_id));
        assert_eq!(prompt, Some(10));
        assert_eq!(completion, Some(5));
    }

    #[tokio::test]
    async fn chat_request_is_converted_to_anthropic() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "upstream-key"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_mock",
                "type": "message",
                "role": "assistant",
                "model": "claude-test",
                "content": [{"type": "text", "text": "Hello from Anthropic"}],
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 20, "output_tokens": 8},
            })))
            .mount(&server)
            .await;

        seed_channel(
            &state,
            "anthropic",
            &format!("{}/v1", server.uri()),
            &["claude-test"],
            10,
            "upstream-key",
        )
        .await;

        let router = app::router(state.clone());
        let (status, body) = send(
            router,
            "/v1/chat/completions",
            Some(&key),
            json!({
                "model": "claude-test",
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 64,
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["choices"][0]["message"]["content"],
            json!("Hello from Anthropic")
        );
        assert_eq!(body["choices"][0]["finish_reason"], json!("stop"));
        assert_eq!(body["usage"]["prompt_tokens"], json!(20));
        assert_eq!(body["usage"]["completion_tokens"], json!(8));

        let requests = server.received_requests().await.expect("requests");
        let upstream_body: Value =
            serde_json::from_slice(&requests[0].body).expect("upstream request json");
        assert_eq!(upstream_body["model"], json!("claude-test"));
        assert_eq!(upstream_body["max_tokens"], json!(64));
        assert_eq!(
            upstream_body["messages"][0]["content"][0]["text"],
            json!("hi")
        );
    }

    #[tokio::test]
    async fn failover_tries_next_channel() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;

        let failing = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": {"message": "boom", "type": "server_error"}
            })))
            .mount(&failing)
            .await;

        let healthy = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat_success_body("recovered")))
            .mount(&healthy)
            .await;

        seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", failing.uri()),
            &["gpt-test"],
            100,
            "bad-key",
        )
        .await;
        let good_id = seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", healthy.uri()),
            &["gpt-test"],
            10,
            "good-key",
        )
        .await;

        let router = app::router(state.clone());
        let (status, body) = send(
            router,
            "/v1/chat/completions",
            Some(&key),
            json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["choices"][0]["message"]["content"], json!("recovered"));

        let (attempts, channel_id, status_code): (i64, Option<i64>, i64) = sqlx::query_as(
            "SELECT attempts, channel_id, status_code FROM usage_logs
             ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&state.db)
        .await
        .expect("usage row");

        assert_eq!(attempts, 2);
        assert_eq!(channel_id, Some(good_id));
        assert_eq!(status_code, 200);
    }

    #[tokio::test]
    async fn client_error_is_not_retried() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;

        let rejecting = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {"message": "bad input", "type": "invalid_request_error"}
            })))
            .mount(&rejecting)
            .await;

        let healthy = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(chat_success_body("should not happen")),
            )
            .mount(&healthy)
            .await;

        let first_id = seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", rejecting.uri()),
            &["gpt-test"],
            100,
            "bad-key",
        )
        .await;
        seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", healthy.uri()),
            &["gpt-test"],
            10,
            "good-key",
        )
        .await;

        let router = app::router(state.clone());
        let (status, body) = send(
            router,
            "/v1/chat/completions",
            Some(&key),
            json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["message"], json!("bad input"));

        let (attempts, channel_id): (i64, Option<i64>) =
            sqlx::query_as("SELECT attempts, channel_id FROM usage_logs ORDER BY id DESC LIMIT 1")
                .fetch_one(&state.db)
                .await
                .expect("usage row");

        assert_eq!(attempts, 1);
        assert_eq!(channel_id, Some(first_id));
        assert_eq!(
            healthy.received_requests().await.unwrap_or_default().len(),
            0
        );
    }

    #[tokio::test]
    async fn anthropic_auth_error_uses_anthropic_shape() {
        let (_temp, state) = test_state().await;
        let router = app::router(state);

        let (status, body) = send(
            router,
            "/v1/messages",
            None,
            json!({"model": "x", "max_tokens": 1, "messages": []}),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["type"], json!("error"));
        assert_eq!(body["error"]["type"], json!("authentication_error"));
    }

    #[tokio::test]
    async fn unknown_model_returns_404() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let router = app::router(state.clone());

        let (status, body) = send(
            router,
            "/v1/chat/completions",
            Some(&key),
            json!({"model": "missing", "messages": []}),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["type"], json!("invalid_request_error"));

        let (attempts, status_code): (i64, i64) =
            sqlx::query_as("SELECT attempts, status_code FROM usage_logs ORDER BY id DESC LIMIT 1")
                .fetch_one(&state.db)
                .await
                .expect("usage row");
        assert_eq!(attempts, 0);
        assert_eq!(status_code, 404);
    }

    #[tokio::test]
    async fn streaming_is_rejected_until_m5() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let router = app::router(state);

        let (status, body) = send(
            router,
            "/v1/chat/completions",
            Some(&key),
            json!({"model": "gpt-test", "stream": true, "messages": []}),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["error"]["message"],
            json!("streaming is not implemented yet")
        );
    }

    #[tokio::test]
    async fn models_list_aggregates_enabled_channels() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;

        seed_channel(
            &state,
            "openai_chat",
            "https://example.invalid/v1",
            &["gpt-a", "gpt-b"],
            10,
            "key-a",
        )
        .await;
        seed_channel(
            &state,
            "anthropic",
            "https://example.invalid/v1",
            &["gpt-b", "claude-c"],
            10,
            "key-b",
        )
        .await;

        let router = app::router(state);
        let (status, body) = send_get(router, "/v1/models", &key).await;

        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = body["data"]
            .as_array()
            .expect("data")
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["claude-c", "gpt-a", "gpt-b"]);
    }
}
