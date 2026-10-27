use std::path::Path;

use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/web/dist"]
struct Assets;

pub async fn fallback(uri: Uri) -> Response {
    let path = uri.path();

    if path.starts_with("/api/") || path.starts_with("/v1/") {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "not found",
                    "type": "not_found",
                }
            })),
        )
            .into_response();
    }

    serve_static(path)
}

fn serve_static(path: &str) -> Response {
    let relative = path.trim_start_matches('/');

    if relative.is_empty() {
        return index_html();
    }

    match Assets::get(relative) {
        Some(file) => embedded_response(relative, file.data.into_owned()),
        None if Path::new(relative).extension().is_none() => index_html(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn index_html() -> Response {
    match Assets::get("index.html") {
        Some(file) => embedded_response("index.html", file.data.into_owned()),
        None => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}

fn embedded_response(path: &str, data: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    let mut response = Response::new(Body::from(data));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}
