use axum::{
    extract::Request,
    http::{header, Method},
    middleware::Next,
    response::Response,
};

use crate::error::AppError;

pub async fn guard(request: Request, next: Next) -> Result<Response, AppError> {
    let method = request.method();
    let mutating = method == Method::POST
        || method == Method::PUT
        || method == Method::PATCH
        || method == Method::DELETE;

    if mutating {
        if let Some(origin) = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
        {
            let origin_host = origin
                .split_once("://")
                .map(|(_, rest)| rest)
                .unwrap_or(origin)
                .split('/')
                .next()
                .unwrap_or_default();
            let host = request
                .headers()
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();

            if !host.is_empty() && origin_host != host {
                return Err(AppError::BadRequest(
                    "cross-origin request rejected".to_string(),
                ));
            }
        }
    }

    Ok(next.run(request).await)
}
