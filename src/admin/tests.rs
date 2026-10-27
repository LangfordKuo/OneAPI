use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use wiremock::{
    matchers::{method, path},
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

async fn test_state() -> (TempDir, AppState) {
    let temp = TempDir::new().expect("temp dir");
    let data_dir = temp.path().to_path_buf();
    let config = Config {
        addr: "127.0.0.1:0".parse().expect("address"),
        data_dir: data_dir.clone(),
        database_url: None,
        database_path: data_dir.join("test.db"),
        log_level: "error".to_string(),
        cookie_secure: false,
    };
    let db = db::connect(&config).await.expect("database");
    let crypto = Arc::new(security::Crypto::load_or_create(&config.data_dir).expect("master key"));
    (
        temp,
        AppState {
            config: Arc::new(config),
            db,
            crypto,
            live: std::sync::Arc::new(crate::metrics::LiveMetrics::default()),
            updates: std::sync::Arc::new(crate::state::UpdateState::default()),
            started_at: chrono::Utc::now(),
        },
    )
}

async fn seed_channel(state: &AppState, base_url: &str) -> i64 {
    let write = ChannelWrite {
        name: "test-chat".to_string(),
        kind: "openai_chat".to_string(),
        base_url: base_url.to_string(),
        api_key_enc: state.crypto.encrypt("upstream-key").expect("encrypt"),
        auth_header: "bearer".to_string(),
        models: json!(["gpt-test"]).to_string(),
        model_mapping: "{}".to_string(),
        extra_headers: "{}".to_string(),
        proxy_url: None,
        priority: 10,
        weight: 1,
        timeout_ms: None,
        enabled: 1,
    };
    models::channel::create(&state.db, &write, now_ms())
        .await
        .expect("channel")
        .id
}

async fn seed_key(state: &AppState) -> String {
    let key = security::generate_api_key();
    let hash = security::api_key_hash(&key);
    let prefix: String = key.chars().take(12).collect();
    models::api_key::create(&state.db, "client", &hash, &prefix, 1, None, now_ms())
        .await
        .expect("api key");
    key
}

async fn admin_token(state: &AppState) -> String {
    let hash = security::hash_password("password123").expect("hash");
    models::admin::create(&state.db, "admin", &hash, now_ms())
        .await
        .expect("admin");
    let token = security::generate_session_token();
    let now = now_ms();
    models::session::create(
        &state.db,
        &security::sha256_hex(&token),
        1,
        now,
        now + 3_600_000,
    )
    .await
    .expect("session");
    token
}

fn chat_response(content: &str) -> Value {
    json!({
        "id": "chatcmpl-admin",
        "object": "chat.completion",
        "created": 1,
        "model": "gpt-test",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
    })
}

async fn send_gateway(
    router: axum::Router,
    path: &str,
    key: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {key}"))
        .body(Body::from(serde_json::to_vec(&body).expect("json")))
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn admin_get(router: axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("cookie", format!("oneapi_session={token}"))
        .body(Body::empty())
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn admin_post(
    router: axum::Router,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("cookie", format!("oneapi_session={token}"))
        .body(Body::from(serde_json::to_vec(&body).expect("json")))
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn admin_post_stream(
    router: axum::Router,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, String) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("cookie", format!("oneapi_session={token}"))
        .body(Body::from(serde_json::to_vec(&body).expect("json")))
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn setup_wizard_creates_admin_and_env() {
    let (_temp, state) = test_state().await;
    let router = app::router(state.clone());

    let (status, body) = admin_get(router.clone(), "/api/setup/status", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["initialized"], json!(false));
    assert_eq!(body["data_dir"], json!(state.config.data_dir_display()));
    assert!(body["env_path"]
        .as_str()
        .unwrap_or_default()
        .ends_with(".env"));
    assert!(body["database"]
        .as_str()
        .unwrap_or_default()
        .ends_with("test.db"));

    let request = Request::builder()
        .method("POST")
        .uri("/api/setup")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "username": "admin",
                "password": "password123",
                "site_name": "OneAPI Test",
            }))
            .expect("json"),
        ))
        .expect("request");
    let response = router.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).to_string())
        .expect("setup issues a session cookie");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["username"], json!("admin"));
    assert_eq!(body["env_written"], json!(true));

    let env_path = state.config.data_dir.join(".env");
    assert!(env_path.exists());
    let contents = std::fs::read_to_string(&env_path).expect("env file");
    assert!(contents.contains("ONEAPI_ADMIN_USER=admin"));
    assert!(contents.contains(&format!(
        "ONEAPI_DATA_DIR={}",
        state.config.data_dir.display()
    )));
    assert!(!contents.contains("password123"));

    let request = Request::builder()
        .method("GET")
        .uri("/api/auth/me")
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request");
    let response = app::router(state.clone())
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let (status, _) = admin_post(
        app::router(state.clone()),
        "/api/setup",
        "",
        json!({"username": "other", "password": "password123"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = admin_get(app::router(state), "/api/setup/status", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["initialized"], json!(true));
}

#[tokio::test]
async fn dashboard_empty_database_returns_zero_metrics() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state);

    let (status, summary) =
        admin_get(router.clone(), "/api/dashboard/summary?range=24h", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["requests"], json!(0));
    assert_eq!(summary["avg_latency_ms"], json!(0.0));

    let (status, timeseries) = admin_get(
        router.clone(),
        "/api/dashboard/timeseries?range=24h",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let points = timeseries["points"].as_array().expect("points");
    assert!(!points.is_empty());
    assert!(points
        .iter()
        .all(|point| point["avg_latency_ms"] == json!(0.0)));

    let (status, breakdown) = admin_get(
        router,
        "/api/dashboard/breakdown?range=24h&dimension=model",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(breakdown["items"], json!([]));
}

#[tokio::test]
async fn logs_and_dashboard_read_usage() {
    let (_temp, state) = test_state().await;
    let key = seed_key(&state).await;
    let token = admin_token(&state).await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response("hello")))
        .mount(&server)
        .await;
    seed_channel(&state, &format!("{}/v1", server.uri())).await;

    let router = app::router(state.clone());
    let (status, _) = send_gateway(
        router.clone(),
        "/v1/chat/completions",
        &key,
        json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, logs) = admin_get(router.clone(), "/api/logs?range=24h", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(logs["total"], json!(1));
    assert_eq!(logs["items"][0]["model"], json!("gpt-test"));
    assert_eq!(logs["items"][0]["source"], json!("gateway"));
    assert_eq!(logs["items"][0]["channel_name"], json!("test-chat"));
    let request_id = logs["items"][0]["request_id"]
        .as_str()
        .expect("request id")
        .to_string();

    let (status, detail) =
        admin_get(router.clone(), &format!("/api/logs/{request_id}"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["total_tokens"], json!(5));

    let (status, summary) =
        admin_get(router.clone(), "/api/dashboard/summary?range=24h", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["requests"], json!(1));
    assert_eq!(summary["success_rate"], json!(1.0));
    assert_eq!(summary["total_tokens"], json!(5));

    let (status, timeseries) = admin_get(
        router.clone(),
        "/api/dashboard/timeseries?range=24h",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let points = timeseries["points"].as_array().expect("points");
    let requests: i64 = points
        .iter()
        .filter_map(|point| point["requests"].as_i64())
        .sum();
    assert_eq!(requests, 1);

    let (status, breakdown) = admin_get(
        router.clone(),
        "/api/dashboard/breakdown?range=24h&dimension=model",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(breakdown["items"][0]["label"], json!("gpt-test"));

    let (status, models) = admin_get(router, "/api/models", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models["models"], json!(["gpt-test"]));
}

#[tokio::test]
async fn playground_proxies_and_logs_both_modes() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response("playground-ok")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let sse = concat!(
        "data: {\"id\":\"chatcmpl-pg\",\"object\":\"chat.completion.chunk\",\"created\":1,",
        "\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"stream-ok\"},",
        "\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-pg\",\"object\":\"chat.completion.chunk\",\"created\":1,",
        "\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;
    seed_channel(&state, &format!("{}/v1", server.uri())).await;

    let router = app::router(state.clone());
    let (status, body) = admin_post(
        router.clone(),
        "/api/playground",
        &token,
        json!({
            "protocol": "chat",
            "model": "gpt-test",
            "messages": [{"role": "user", "content": "hi"}],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["choices"][0]["message"]["content"],
        json!("playground-ok")
    );

    let (status, body) = admin_post_stream(
        router,
        "/api/playground",
        &token,
        json!({
            "protocol": "chat",
            "model": "gpt-test",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("stream-ok"));
    assert!(body.contains("[DONE]"));

    let rows: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT source, api_key_id FROM usage_logs ORDER BY id")
            .fetch_all(&state.db)
            .await
            .expect("usage rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "playground");
    assert_eq!(rows[0].1, None);
    assert_eq!(rows[1].0, "playground");
    assert_eq!(rows[1].1, None);
}

#[tokio::test]
async fn channel_probe_works_before_saving() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response("pong")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "gpt-a"}]})))
        .mount(&server)
        .await;

    let router = app::router(state.clone());
    let draft = json!({
        "name": "draft",
        "kind": "openai_chat",
        "base_url": format!("{}/v1", server.uri()),
        "api_key": "upstream-key",
        "models": ["gpt-test"],
    });

    let (status, body) =
        admin_post(router.clone(), "/api/channels/test", &token, draft.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));

    let (status, body) = admin_post(router, "/api/channels/fetch-models", &token, draft).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["models"], json!(["gpt-a"]));

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM channels")
        .fetch_one(&state.db)
        .await
        .expect("channel count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn channel_weight_is_bounded() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state);

    let (status, _) = admin_post(
        router,
        "/api/channels",
        &token,
        json!({
            "name": "too-heavy",
            "kind": "openai_chat",
            "base_url": "https://example.invalid/v1",
            "api_key": "upstream-key",
            "models": ["gpt-test"],
            "weight": 1001,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}
