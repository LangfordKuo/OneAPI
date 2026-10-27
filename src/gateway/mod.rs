use std::collections::BTreeSet;

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

use crate::{error::AppError, models, protocol::Protocol, state::AppState, upstream};

use auth::ApiKeyAuth;

pub mod auth;
pub mod error;
pub mod execute;
pub mod responses_state;
pub mod select;
pub mod stream;
pub mod usage;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/responses", post(responses))
        .route(
            "/v1/responses/{id}",
            get(get_response).delete(delete_response),
        )
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
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

/// Anthropic `count_tokens` endpoint. When an Anthropic upstream is available we
/// forward the request as-is; otherwise we return a clearly approximate estimate
/// so clients that probe context size keep working across providers.
async fn count_tokens(
    State(state): State<AppState>,
    _auth: ApiKeyAuth,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, AppError> {
    if let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let channels = state.cache.channels(&state.db).await?;
        if let Some(candidate) = select::candidates(channels, model)
            .into_iter()
            .find(|candidate| candidate.channel.kind == "anthropic")
        {
            let defaults = models::setting::GatewayDefaults::load(&state.db).await?;
            let mut request_body = body.clone();
            if let Some(object) = request_body.as_object_mut() {
                object.insert("model".into(), json!(candidate.upstream_model));
                object.remove("stream");
            }

            let client =
                match upstream::build_client(&candidate.channel, defaults.nonstream_timeout_ms) {
                    Ok(client) => client,
                    Err(error) => {
                        return Ok(error::client_error(
                            Protocol::Anthropic,
                            StatusCode::BAD_GATEWAY,
                            error.to_string(),
                            "upstream_configuration_error",
                        ))
                    }
                };
            let url = format!(
                "{}/messages/count_tokens",
                candidate.channel.base_url.trim_end_matches('/')
            );
            let request = match upstream::apply_auth(
                client.post(url).json(&request_body),
                &candidate.channel,
                &state.crypto,
                &headers,
            ) {
                Ok(request) => request,
                Err(error) => {
                    return Ok(error::client_error(
                        Protocol::Anthropic,
                        StatusCode::BAD_GATEWAY,
                        error.to_string(),
                        "upstream_configuration_error",
                    ))
                }
            };

            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    return Ok((status, [(header::CONTENT_TYPE, "application/json")], text)
                        .into_response());
                }
                Err(error) => {
                    return Ok(error::client_error(
                        Protocol::Anthropic,
                        StatusCode::BAD_GATEWAY,
                        error.to_string(),
                        "upstream_unreachable",
                    ))
                }
            }
        }
    }

    Ok(Json(json!({ "input_tokens": estimate_input_tokens(&body) })).into_response())
}

fn estimate_input_tokens(body: &Value) -> u64 {
    fn char_count(value: &Value) -> u64 {
        match value {
            Value::String(text) => text.chars().count() as u64,
            Value::Array(items) => items.iter().map(char_count).sum(),
            Value::Object(object) => object
                .iter()
                .filter(|(key, _)| key.as_str() != "model" && key.as_str() != "stream")
                .map(|(_, value)| char_count(value))
                .sum(),
            _ => 0,
        }
    }

    char_count(body).div_ceil(4).max(1)
}

async fn get_response(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    match responses_state::get(&state, auth.key.id, &id).await? {
        Some(stored) => {
            let value: Value = serde_json::from_str(&stored.response_json)
                .map_err(|error| AppError::Internal(error.into()))?;
            Ok(Json(value).into_response())
        }
        None => Ok(error::client_error(
            Protocol::Responses,
            StatusCode::NOT_FOUND,
            "response not found",
            "not_found",
        )),
    }
}

async fn delete_response(
    State(state): State<AppState>,
    auth: ApiKeyAuth,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    if responses_state::delete(&state, auth.key.id, &id).await? {
        Ok(Json(json!({
            "id": id,
            "object": "response.deleted",
            "deleted": true,
        }))
        .into_response())
    } else {
        Ok(error::client_error(
            Protocol::Responses,
            StatusCode::NOT_FOUND,
            "response not found",
            "not_found",
        ))
    }
}

async fn list_models(
    State(state): State<AppState>,
    _auth: ApiKeyAuth,
) -> Result<Json<Value>, AppError> {
    let models = available_models(&state).await?;
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
    let models = available_models(&state).await?;
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

async fn available_models(state: &AppState) -> sqlx::Result<Vec<String>> {
    let channels = state.cache.channels(&state.db).await?;
    let mut result = BTreeSet::new();
    let mut has_wildcard = false;

    for channel in channels {
        if channel.enabled == 0 {
            continue;
        }
        let channel_models: Vec<String> = serde_json::from_str(&channel.models).unwrap_or_default();
        for model in channel_models {
            if model == "*" {
                has_wildcard = true;
            } else if !model.is_empty() {
                result.insert(model);
            }
        }
    }

    // Wildcard channels accept any model, so surface models seen in recent
    // traffic to keep model pickers useful.
    if has_wildcard {
        let since = models::now_ms() - 30 * 24 * 60 * 60 * 1000;
        for model in models::usage_log::recent_models(&state.db, since, 200).await? {
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
            trust_proxy: false,
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
            live: std::sync::Arc::new(crate::metrics::LiveMetrics::default()),
            updates: std::sync::Arc::new(crate::state::UpdateState::default()),
            login_limiter: std::sync::Arc::new(crate::admin::LoginLimiter::default()),
            clients: std::sync::Arc::new(crate::upstream::ClientCache::default()),
            cache: std::sync::Arc::new(crate::state::RuntimeCache::default()),
            usage_writer: None,
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

    async fn send_delete(router: axum::Router, path: &str, key: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("DELETE")
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

    async fn send_stream(
        router: axum::Router,
        path: &str,
        key: &str,
        body: Value,
    ) -> (StatusCode, String) {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {key}"))
            .body(Body::from(serde_json::to_vec(&body).expect("request json")))
            .expect("request");
        let response = router.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, String::from_utf8_lossy(&bytes).to_string())
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
        assert!(state.live.snapshot().global_tok_s > 0.0);
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
    async fn streaming_is_passed_through_and_logged() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        let sse = concat!(
            "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},",
            "\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-test\",\"choices\":[],",
            "\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1,\"total_tokens\":4}}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", server.uri()),
            &["gpt-test"],
            10,
            "upstream-key",
        )
        .await;

        let router = app::router(state.clone());
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {key}"))
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-test",
                    "stream": true,
                    "messages": [{"role": "user", "content": "hi"}],
                }))
                .expect("request json"),
            ))
            .expect("request");

        let response = router.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("\"content\":\"hello\""));
        assert!(body.contains("data: [DONE]"));
        // The usage-only chunk was injected upstream but filtered for the client.
        assert!(!body.contains("\"usage\""));

        let (stream, attempts, prompt): (i64, i64, Option<i64>) = sqlx::query_as(
            "SELECT stream, attempts, prompt_tokens FROM usage_logs
             ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&state.db)
        .await
        .expect("usage row");
        assert_eq!(stream, 1);
        assert_eq!(attempts, 1);
        assert_eq!(prompt, Some(3));

        let requests = server.received_requests().await.expect("requests");
        let upstream: Value = serde_json::from_slice(&requests[0].body).expect("upstream json");
        assert_eq!(upstream["stream"], json!(true));
        assert_eq!(upstream["stream_options"]["include_usage"], json!(true));
    }

    #[tokio::test]
    async fn streaming_failover_after_first_error_event() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;

        let failing = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "event: error\ndata: {\"error\":{\"message\":\"busy\",\"type\":\"overloaded_error\"}}\n\n",
                "text/event-stream",
            ))
            .mount(&failing)
            .await;

        let healthy = MockServer::start().await;
        let sse = concat!(
            "data: {\"id\":\"chatcmpl-2\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"recovered\"},",
            "\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-2\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},",
            "\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
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
        let healthy_id = seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", healthy.uri()),
            &["gpt-test"],
            10,
            "good-key",
        )
        .await;

        let router = app::router(state.clone());
        let (status, body) = send_stream(
            router,
            "/v1/chat/completions",
            &key,
            json!({
                "model": "gpt-test",
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}],
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"content\":\"recovered\""));
        assert!(body.contains("data: [DONE]"));

        let (attempts, channel, stream): (i64, Option<i64>, i64) = sqlx::query_as(
            "SELECT attempts, channel_id, stream FROM usage_logs
             ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&state.db)
        .await
        .expect("usage row");
        assert_eq!(attempts, 2);
        assert_eq!(channel, Some(healthy_id));
        assert_eq!(stream, 1);
    }

    #[tokio::test]
    async fn responses_streaming_is_stored_and_fetchable() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        let sse = concat!(
            "data: {\"id\":\"chatcmpl-3\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"streamed answer\"},",
            "\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-3\",\"object\":\"chat.completion.chunk\",\"created\":1,",
            "\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},",
            "\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", server.uri()),
            &["gpt-test"],
            10,
            "upstream-key",
        )
        .await;

        let router = app::router(state.clone());
        let (status, body) = send_stream(
            router.clone(),
            "/v1/responses",
            &key,
            json!({"model": "gpt-test", "input": "hello", "stream": true}),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("event: response.created"));
        assert!(body.contains("event: response.output_text.delta"));
        assert!(body.contains("event: response.completed"));
        assert!(body.contains("\"text\":\"streamed answer\""));

        let mut response_id = None;
        for line in body.lines() {
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(id) = value
                .get("response")
                .and_then(|response| response.get("id"))
                .and_then(Value::as_str)
                .filter(|id| id.starts_with("resp_"))
            {
                response_id = Some(id.to_string());
            }
        }
        let response_id = response_id.expect("local response id");

        let stored: (String, String) = sqlx::query_as(
            "SELECT id, status FROM responses WHERE id = ? AND api_key_id IS NOT NULL",
        )
        .bind(&response_id)
        .fetch_one(&state.db)
        .await
        .expect("stored response");
        assert_eq!(stored.0, response_id);
        assert_eq!(stored.1, "completed");

        let (status, stored_body) =
            send_get(router, &format!("/v1/responses/{response_id}"), &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            stored_body["output"][0]["content"][0]["text"],
            json!("streamed answer")
        );
    }

    #[tokio::test]
    async fn request_bodies_are_stored_when_enabled() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat_success_body("hello")))
            .mount(&server)
            .await;
        seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", server.uri()),
            &["gpt-test"],
            10,
            "upstream-key",
        )
        .await;

        let router = app::router(state.clone());
        let body = json!({
            "model": "gpt-test",
            "messages": [{"role": "user", "content": "store me"}],
        });
        let (status, _) = send(
            router.clone(),
            "/v1/chat/completions",
            Some(&key),
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_details")
            .fetch_one(&state.db)
            .await
            .expect("count");
        assert_eq!(count, 0);

        models::setting::upsert(&state.db, "store_request_bodies", "true")
            .await
            .expect("setting");
        state.cache.invalidate_defaults();
        let (status, _) = send(router, "/v1/chat/completions", Some(&key), body).await;
        assert_eq!(status, StatusCode::OK);

        let (request_body, response_body): (String, String) = sqlx::query_as(
            "SELECT request_body, response_body FROM request_details
             ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(&state.db)
        .await
        .expect("request detail");
        assert!(request_body.contains("store me"));
        assert!(response_body.contains("hello"));
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

    #[tokio::test]
    async fn responses_history_is_expanded_and_scoped() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let other_key = seed_key(&state).await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(chat_success_body("first answer")),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(chat_success_body("second answer")),
            )
            .mount(&server)
            .await;

        seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", server.uri()),
            &["gpt-test"],
            10,
            "upstream-key",
        )
        .await;

        let router = app::router(state.clone());
        let (status, first) = send(
            router.clone(),
            "/v1/responses",
            Some(&key),
            json!({"model": "gpt-test", "input": "hello"}),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["object"], json!("response"));
        assert_eq!(first["store"], json!(true));
        assert_eq!(
            first["output"][0]["content"][0]["text"],
            json!("first answer")
        );
        let first_id = first["id"].as_str().expect("response id").to_string();
        assert!(first_id.starts_with("resp_"));

        let (status, second) = send(
            router.clone(),
            "/v1/responses",
            Some(&key),
            json!({
                "model": "gpt-test",
                "previous_response_id": first_id.clone(),
                "input": "and again",
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            second["output"][0]["content"][0]["text"],
            json!("second answer")
        );

        let requests = server.received_requests().await.expect("requests");
        assert_eq!(requests.len(), 2);
        let second_request: Value =
            serde_json::from_slice(&requests[1].body).expect("second upstream request");
        let messages = second_request["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["content"], json!("hello"));
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[1]["content"], json!("first answer"));
        assert_eq!(messages[2]["role"], json!("user"));
        assert_eq!(messages[2]["content"], json!("and again"));

        let (status, stored) =
            send_get(router.clone(), &format!("/v1/responses/{first_id}"), &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stored["id"], json!(first_id.as_str()));

        let (status, _) = send_get(
            router.clone(),
            &format!("/v1/responses/{first_id}"),
            &other_key,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = send(
            router.clone(),
            "/v1/responses",
            Some(&other_key),
            json!({
                "model": "gpt-test",
                "previous_response_id": first_id.clone(),
                "input": "steal",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, deleted) =
            send_delete(router.clone(), &format!("/v1/responses/{first_id}"), &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(deleted["deleted"], json!(true));

        let (status, _) = send_get(router, &format!("/v1/responses/{first_id}"), &key).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn responses_store_false_cannot_be_referenced() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat_success_body("temporary")))
            .mount(&server)
            .await;

        seed_channel(
            &state,
            "openai_chat",
            &format!("{}/v1", server.uri()),
            &["gpt-test"],
            10,
            "upstream-key",
        )
        .await;

        let router = app::router(state);
        let (status, first) = send(
            router.clone(),
            "/v1/responses",
            Some(&key),
            json!({"model": "gpt-test", "input": "hello", "store": false}),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["store"], json!(false));
        let first_id = first["id"].as_str().expect("response id").to_string();

        let (status, body) = send(
            router,
            "/v1/responses",
            Some(&key),
            json!({
                "model": "gpt-test",
                "previous_response_id": first_id,
                "input": "again",
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not found"));
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            1
        );
    }

    #[tokio::test]
    async fn count_tokens_estimates_without_anthropic_upstream() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let router = app::router(state);

        let (status, body) = send(
            router,
            "/v1/messages/count_tokens",
            Some(&key),
            json!({
                "model": "claude-test",
                "messages": [{"role": "user", "content": "12345678"}],
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(body["input_tokens"].as_u64().unwrap_or(0) >= 2);
    }

    #[tokio::test]
    async fn count_tokens_is_forwarded_to_anthropic_upstream() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages/count_tokens"))
            .and(header("x-api-key", "upstream-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"input_tokens": 42})))
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

        let router = app::router(state);
        let (status, body) = send(
            router,
            "/v1/messages/count_tokens",
            Some(&key),
            json!({
                "model": "claude-test",
                "messages": [{"role": "user", "content": "hi"}],
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["input_tokens"], json!(42));

        let requests = server.received_requests().await.expect("requests");
        let upstream: Value =
            serde_json::from_slice(&requests[0].body).expect("upstream request json");
        assert_eq!(upstream["model"], json!("claude-test"));
        assert!(upstream.get("stream").is_none());
    }

    #[tokio::test]
    async fn wildcard_channels_list_recent_models() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;

        seed_channel(
            &state,
            "openai_chat",
            "https://example.invalid/v1",
            &["*"],
            10,
            "upstream-key",
        )
        .await;

        sqlx::query(
            "INSERT INTO usage_logs
             (request_id, source, model, inbound_protocol, upstream_protocol, stream,
              status_code, attempts, latency_ms, created_at)
             VALUES ('req-seen', 'gateway', 'gpt-seen', 'chat', 'chat', 0, 200, 1, 10, ?)",
        )
        .bind(models::now_ms())
        .execute(&state.db)
        .await
        .expect("usage log");

        let router = app::router(state);
        let (status, body) = send_get(router, "/v1/models", &key).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"][0]["id"], json!("gpt-seen"));
    }

    #[tokio::test]
    async fn default_max_tokens_setting_is_used_for_anthropic_upstream() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_mock",
                "type": "message",
                "role": "assistant",
                "model": "claude-test",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 1, "output_tokens": 1},
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
        models::setting::upsert(&state.db, "default_max_tokens", "2048")
            .await
            .expect("setting");

        let router = app::router(state);
        let (status, _) = send(
            router,
            "/v1/chat/completions",
            Some(&key),
            json!({
                "model": "claude-test",
                "messages": [{"role": "user", "content": "hi"}],
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let requests = server.received_requests().await.expect("requests");
        let upstream: Value =
            serde_json::from_slice(&requests[0].body).expect("upstream request json");
        assert_eq!(upstream["max_tokens"], json!(2048));
    }

    #[tokio::test]
    async fn responses_expansion_rejects_oversized_history() {
        let (_temp, state) = test_state().await;
        let key = seed_key(&state).await;
        let key_id = models::api_key::find_by_hash(&state.db, &security::api_key_hash(&key))
            .await
            .expect("api key lookup")
            .expect("api key")
            .id;

        let items: Vec<Value> = (0..2_001)
            .map(|index| json!({"type": "message", "role": "user", "content": index.to_string()}))
            .collect();
        let request_json = json!({"model": "gpt-test", "input": items}).to_string();

        sqlx::query(
            "INSERT INTO responses
             (id, api_key_id, model, request_json, response_json, status, created_at)
             VALUES ('resp_big', ?, 'gpt-test', ?, '{\"output\":[]}', 'completed', ?)",
        )
        .bind(key_id)
        .bind(&request_json)
        .bind(models::now_ms())
        .execute(&state.db)
        .await
        .expect("stored response");

        let router = app::router(state);
        let (status, body) = send(
            router,
            "/v1/responses",
            Some(&key),
            json!({
                "model": "gpt-test",
                "previous_response_id": "resp_big",
                "input": "again",
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("expanded conversation"));
    }
}
