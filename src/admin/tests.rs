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
    app, client_ip,
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
        trust_proxy: false,
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
            login_limiter: std::sync::Arc::new(crate::admin::LoginLimiter::default()),
            clients: std::sync::Arc::new(crate::upstream::ClientCache::default()),
            cache: std::sync::Arc::new(crate::state::RuntimeCache::default()),
            probes: std::sync::Arc::new(crate::state::ChannelProbes::default()),
            usage_writer: None,
            started_at: chrono::Utc::now(),
        },
    )
}

async fn seed_channel(state: &AppState, base_url: &str) -> i64 {
    let write = ChannelWrite {
        name: "test-chat".to_string(),
        kind: "openai_chat".to_string(),
        kinds: r#"["openai_chat"]"#.to_string(),
        fallback_kind: "openai_chat".to_string(),
        protocol_configs: "[]".to_string(),
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

async fn gateway_get(router: axum::Router, path: &str, key: &str) -> (StatusCode, Value) {
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

async fn admin_put(
    router: axum::Router,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("PUT")
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

async fn admin_put_with_origin(
    router: axum::Router,
    path: &str,
    token: &str,
    body: Value,
    origin: &str,
    host: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .header("cookie", format!("oneapi_session={token}"))
        .header("origin", origin)
        .header("host", host)
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

async fn admin_post_with_client_ip(
    router: axum::Router,
    path: &str,
    body: Value,
    header: &str,
    ip: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header(header, ip)
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
    let latency_ms = logs["items"][0]["latency_ms"].as_i64().expect("latency_ms");
    if latency_ms > 0 {
        let expected = 2.0 * 1000.0 / latency_ms as f64;
        let speed = logs["items"][0]["tokens_per_second"]
            .as_f64()
            .expect("tokens_per_second");
        assert!((speed - expected).abs() < 0.001);
    } else {
        assert!(logs["items"][0]["tokens_per_second"].is_null());
    }
    let request_id = logs["items"][0]["request_id"]
        .as_str()
        .expect("request id")
        .to_string();

    let (status, detail) =
        admin_get(router.clone(), &format!("/api/logs/{request_id}"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["total_tokens"], json!(5));
    assert_eq!(
        detail["tokens_per_second"], logs["items"][0]["tokens_per_second"],
        "detail and list must report the same speed"
    );

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

    let (status, models) = admin_get(router.clone(), "/api/models", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models["models"], json!(["gpt-test"]));

    let (status, _) = send_gateway(
        router.clone(),
        "/v1/chat/completions",
        &key,
        json!({"model": "missing-model", "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, errors) =
        admin_get(router.clone(), "/api/logs?range=24h&status=error", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(errors["total"], json!(1));
    assert_eq!(errors["items"][0]["status_code"], json!(404));

    let (status, successes) =
        admin_get(router.clone(), "/api/logs?range=24h&status=success", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(successes["total"], json!(1));
    assert_eq!(successes["items"][0]["status_code"], json!(200));

    let (status, filtered) = admin_get(
        router.clone(),
        "/api/logs?range=24h&model=missing-model",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["total"], json!(1));
    assert_eq!(filtered["items"][0]["model"], json!("missing-model"));

    let (status, none) =
        admin_get(router, "/api/logs?range=24h&model=does-not-exist", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(none["total"], json!(0));
    assert_eq!(none["items"].as_array().map(Vec::len), Some(0));
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
async fn login_is_rate_limited_after_repeated_failures() {
    let (_temp, state) = test_state().await;
    admin_token(&state).await;
    let router = app::router(state);

    for _ in 0..5 {
        let (status, _) = admin_post(
            router.clone(),
            "/api/auth/login",
            "",
            json!({"username": "admin", "password": "wrong-password"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let (status, _) = admin_post(
        router,
        "/api/auth/login",
        "",
        json!({"username": "admin", "password": "password123"}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn client_ip_header_setting_is_validated_and_saved() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state.clone());

    let (status, body) = admin_put(
        router.clone(),
        "/api/settings",
        &token,
        json!({"client_ip_header": "CF-Connecting-IP"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["client_ip_header"], json!("cf-connecting-ip"));

    let (status, _) = admin_put(
        router.clone(),
        "/api/settings",
        &token,
        json!({"client_ip_header": "bad header"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, body) = admin_get(router, "/api/settings", &token).await;
    assert_eq!(body["client_ip_header"], json!("cf-connecting-ip"));
}

#[tokio::test]
async fn site_domain_setting_is_normalized_and_relaxes_csrf_after_proxy() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state.clone());

    let (status, body) = admin_put(
        router.clone(),
        "/api/settings",
        &token,
        json!({"site_domain": "https://API.Example.com/dashboard"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["site_domain"], json!("api.example.com"));

    let (status, _) = admin_put(
        router.clone(),
        "/api/settings",
        &token,
        json!({"site_domain": "not a domain"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The proxied Origin is accepted even though Host points at the backend.
    let (status, body) = admin_put_with_origin(
        router.clone(),
        "/api/settings",
        &token,
        json!({}),
        "https://api.example.com",
        "127.0.0.1:3000",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["site_domain"], json!("api.example.com"));

    // Any other Origin is still rejected.
    let (status, _) = admin_put_with_origin(
        router,
        "/api/settings",
        &token,
        json!({}),
        "https://evil.example.com",
        "127.0.0.1:3000",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn login_limiter_uses_configured_client_ip_header() {
    let (_temp, state) = test_state().await;
    admin_token(&state).await;
    models::setting::upsert(&state.db, client_ip::SETTING_KEY, "cf-connecting-ip")
        .await
        .expect("setting");
    let router = app::router(state);

    for index in 0..5 {
        let (status, _) = admin_post_with_client_ip(
            router.clone(),
            "/api/auth/login",
            json!({"username": "admin", "password": "wrong-password"}),
            "cf-connecting-ip",
            &format!("203.0.113.{index}"),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let (status, _) = admin_post_with_client_ip(
        router.clone(),
        "/api/auth/login",
        json!({"username": "admin", "password": "password123"}),
        "cf-connecting-ip",
        "203.0.113.99",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    for _ in 0..5 {
        let (status, _) = admin_post_with_client_ip(
            router.clone(),
            "/api/auth/login",
            json!({"username": "admin", "password": "wrong-password"}),
            "cf-connecting-ip",
            "198.51.100.7",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let (status, _) = admin_post_with_client_ip(
        router,
        "/api/auth/login",
        json!({"username": "admin", "password": "password123"}),
        "cf-connecting-ip",
        "198.51.100.7",
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn channel_supports_multiple_protocols() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state);

    let (status, body) = admin_post(
        router.clone(),
        "/api/channels",
        &token,
        json!({
            "name": "multi",
            "kinds": ["openai_chat", "openai_responses", "anthropic"],
            "fallback_kind": "openai_responses",
            "base_url": "https://example.invalid/v1",
            "api_key": "upstream-key",
            "models": ["multi-test"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        body["kinds"],
        json!(["openai_chat", "openai_responses", "anthropic"])
    );
    assert_eq!(body["fallback_kind"], json!("openai_responses"));
    let channel_id = body["id"].as_i64().expect("id");

    let (status, body) = admin_put(
        router.clone(),
        &format!("/api/channels/{channel_id}"),
        &token,
        json!({
            "name": "multi",
            "kinds": ["anthropic"],
            "fallback_kind": "anthropic",
            "base_url": "https://example.invalid/v1",
            "models": ["multi-test"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kinds"], json!(["anthropic"]));
    assert_eq!(body["fallback_kind"], json!("anthropic"));

    let (status, _) = admin_put(
        router,
        &format!("/api/channels/{channel_id}"),
        &token,
        json!({
            "name": "multi",
            "kinds": ["openai_chat"],
            "fallback_kind": "anthropic",
            "base_url": "https://example.invalid/v1",
            "models": ["multi-test"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
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
async fn channel_protocols_use_their_own_upstream_configs() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let server = MockServer::start().await;

    for (path_value, body) in [
        ("/chat/v1/chat/completions", chat_response("chat")),
        ("/resp/v1/responses", json!({"id": "resp_1", "output": []})),
        (
            "/ant/v1/messages",
            json!({"id": "msg_1", "content": [{"type": "text", "text": "ant"}]}),
        ),
    ] {
        Mock::given(method("POST"))
            .and(path(path_value))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }

    let draft = json!({
        "name": "three-endpoints",
        "protocols": [
            {
                "protocol": "openai_chat",
                "mode": "native",
                "base_url": format!("{}/chat/v1", server.uri()),
                "api_key": "key-chat",
                "models": ["gpt-test"],
            },
            {
                "protocol": "openai_responses",
                "mode": "native",
                "base_url": format!("{}/resp/v1", server.uri()),
                "api_key": "key-resp",
                "models": ["gpt-test"],
            },
            {
                "protocol": "anthropic",
                "mode": "native",
                "base_url": format!("{}/ant/v1", server.uri()),
                "api_key": "key-ant",
                "models": ["claude-test"],
            },
        ],
    });

    let router = app::router(state.clone());
    for protocol in ["openai_chat", "openai_responses", "anthropic"] {
        let mut payload = draft.clone();
        payload["probe_protocol"] = json!(protocol);
        let (status, body) =
            admin_post(router.clone(), "/api/channels/test", &token, payload).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["ok"],
            json!(true),
            "{protocol} probe failed: {}",
            body["message"]
        );
    }

    let (status, body) = admin_post(router, "/api/channels", &token, draft).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["protocols"][0]["mode"], json!("native"));
    assert_eq!(body["protocols"][1]["mode"], json!("native"));
    assert_eq!(body["protocols"][2]["mode"], json!("native"));
    assert_eq!(body["protocols"][0]["api_key_masked"], json!("••••chat"));
    assert_eq!(body["protocols"][1]["api_key_masked"], json!("••••resp"));
    assert_eq!(body["protocols"][2]["api_key_masked"], json!("••••-ant"));
    assert_eq!(
        body["protocols"][1]["base_url"],
        json!(format!("{}/resp/v1", server.uri()))
    );
}

#[tokio::test]
async fn converted_protocol_reuses_the_target_native_config() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response("converted")))
        .mount(&server)
        .await;

    let router = app::router(state.clone());
    let (status, body) = admin_post(
        router,
        "/api/channels",
        &token,
        json!({
            "name": "converted",
            "protocols": [
                {
                    "protocol": "openai_chat",
                    "mode": "native",
                    "base_url": format!("{}/chat/v1", server.uri()),
                    "api_key": "chat-key",
                    "models": ["gpt-test"],
                },
                {
                    "protocol": "openai_responses",
                    "mode": "convert",
                    "target": "openai_chat",
                },
                {
                    "protocol": "anthropic",
                    "mode": "convert",
                    "target": "openai_chat",
                },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["protocols"][1]["target"], json!("openai_chat"));
    assert_eq!(body["protocols"][2]["target"], json!("openai_chat"));

    let key = seed_key(&state).await;
    let router = app::router(state);
    let (status, body) = send_gateway(
        router,
        "/v1/messages",
        &key,
        json!({
            "model": "gpt-test",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"][0]["text"], json!("converted"));
}

#[tokio::test]
async fn conversion_target_must_be_a_native_protocol() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state);

    let (status, _) = admin_post(
        router,
        "/api/channels",
        &token,
        json!({
            "name": "bad-target",
            "protocols": [
                {
                    "protocol": "openai_chat",
                    "mode": "native",
                    "base_url": "https://example.invalid/v1",
                    "api_key": "chat-key",
                    "models": ["gpt-test"],
                },
                {
                    "protocol": "openai_responses",
                    "mode": "convert",
                    "target": "anthropic",
                },
                {
                    "protocol": "anthropic",
                    "mode": "convert",
                    "target": "openai_chat",
                },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_key_restrictions_are_validated_and_updated() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state.clone());

    let (status, channel) = admin_post(
        router.clone(),
        "/api/channels",
        &token,
        json!({
            "name": "restriction-target",
            "kind": "openai_chat",
            "base_url": "https://example.invalid/v1",
            "api_key": "upstream-key",
            "models": ["gpt-test"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let channel_id = channel["id"].as_i64().expect("channel id");

    let (status, body) = admin_post(
        router.clone(),
        "/api/keys",
        &token,
        json!({
            "name": "restricted",
            "allowed_channel_ids": [channel_id],
            "allowed_models": ["gpt-test", "*", "gpt-test"],
            "token_limit": 1000,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["allowed_channel_ids"], json!([channel_id]));
    assert_eq!(body["allowed_models"], json!(["gpt-test", "*"]));
    assert_eq!(body["token_limit"], json!(1000));
    assert_eq!(body["token_usage"], json!(0));
    let key_id = body["id"].as_i64().expect("key id");

    let (status, _) = admin_post(
        router.clone(),
        "/api/keys",
        &token,
        json!({"name": "bad-channel", "allowed_channel_ids": [9999]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = admin_post(
        router.clone(),
        "/api/keys",
        &token,
        json!({"name": "bad-limit", "token_limit": -1}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = admin_put(
        router,
        &format!("/api/keys/{key_id}"),
        &token,
        json!({
            "allowed_channel_ids": [],
            "allowed_models": [],
            "token_limit": null,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["allowed_channel_ids"], json!([]));
    assert_eq!(body["allowed_models"], json!([]));
    assert_eq!(body["token_limit"], json!(null));
}

#[tokio::test]
async fn api_key_model_restriction_blocks_unlisted_models() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let router = app::router(state.clone());

    admin_post(
        router.clone(),
        "/api/channels",
        &token,
        json!({
            "name": "catalog",
            "kind": "openai_chat",
            "base_url": "https://example.invalid/v1",
            "api_key": "upstream-key",
            "models": ["gpt-test"],
        }),
    )
    .await;

    let (_, limited_key) = admin_post(
        router.clone(),
        "/api/keys",
        &token,
        json!({"name": "model-limited", "allowed_models": ["other-model"]}),
    )
    .await;
    let limited = limited_key["key"].as_str().expect("key").to_string();

    let (status, body) = send_gateway(
        router.clone(),
        "/v1/chat/completions",
        &limited,
        json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["type"], json!("permission_error"));

    let (status, models) = gateway_get(router.clone(), "/v1/models", &limited).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models["data"], json!([]));

    let (_, allowed_key) = admin_post(
        router.clone(),
        "/api/keys",
        &token,
        json!({"name": "model-allowed", "allowed_models": ["gpt-test"]}),
    )
    .await;
    let allowed = allowed_key["key"].as_str().expect("key").to_string();
    let (status, models) = gateway_get(router, "/v1/models", &allowed).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models["data"][0]["id"], json!("gpt-test"));
}

#[tokio::test]
async fn api_key_channel_restriction_limits_selection() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let allowed_server = MockServer::start().await;
    let blocked_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response("from-allowed")))
        .mount(&allowed_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response("from-blocked")))
        .mount(&blocked_server)
        .await;

    let router = app::router(state.clone());
    let (_, allowed_channel) = admin_post(
        router.clone(),
        "/api/channels",
        &token,
        json!({
            "name": "allowed",
            "kind": "openai_chat",
            "base_url": format!("{}/v1", allowed_server.uri()),
            "api_key": "allowed-key",
            "models": ["gpt-test"],
            "priority": 0,
        }),
    )
    .await;
    let allowed_id = allowed_channel["id"].as_i64().expect("channel id");

    admin_post(
        router.clone(),
        "/api/channels",
        &token,
        json!({
            "name": "blocked",
            "kind": "openai_chat",
            "base_url": format!("{}/v1", blocked_server.uri()),
            "api_key": "blocked-key",
            "models": ["gpt-test"],
            "priority": 100,
        }),
    )
    .await;

    let (_, key) = admin_post(
        router.clone(),
        "/api/keys",
        &token,
        json!({"name": "channel-limited", "allowed_channel_ids": [allowed_id]}),
    )
    .await;
    let key = key["key"].as_str().expect("key").to_string();

    let (status, body) = send_gateway(
        router,
        "/v1/chat/completions",
        &key,
        json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["choices"][0]["message"]["content"],
        json!("from-allowed")
    );
}

#[tokio::test]
async fn api_key_token_limit_disables_after_usage() {
    let (_temp, state) = test_state().await;
    let token = admin_token(&state).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_response("ok")))
        .mount(&server)
        .await;

    let router = app::router(state.clone());
    admin_post(
        router.clone(),
        "/api/channels",
        &token,
        json!({
            "name": "quota",
            "kind": "openai_chat",
            "base_url": format!("{}/v1", server.uri()),
            "api_key": "upstream-key",
            "models": ["gpt-test"],
        }),
    )
    .await;

    let (_, key) = admin_post(
        router.clone(),
        "/api/keys",
        &token,
        json!({"name": "quota-key", "token_limit": 8}),
    )
    .await;
    let key = key["key"].as_str().expect("key").to_string();

    for _ in 0..2 {
        let (status, _) = send_gateway(
            router.clone(),
            "/v1/chat/completions",
            &key,
            json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send_gateway(
        router.clone(),
        "/v1/chat/completions",
        &key,
        json!({"model": "gpt-test", "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["type"], json!("permission_error"));

    let (status, keys) = admin_get(router, "/api/keys", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys[0]["token_usage"], json!(10));
    assert_eq!(keys[0]["token_limit"], json!(8));
    assert_eq!(keys[0]["enabled"], json!(false));
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
