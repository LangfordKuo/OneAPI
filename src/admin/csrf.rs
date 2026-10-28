use axum::{
    extract::{Request, State},
    http::{header, Method},
    middleware::Next,
    response::Response,
};

use crate::{error::AppError, state::AppState};

use super::settings::normalize_authority;

pub async fn guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
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
            let forwarded_host = state.config.trust_proxy.then(|| {
                request
                    .headers()
                    .get("x-forwarded-host")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(',').next())
                    .map(str::trim)
            });
            let host = forwarded_host
                .flatten()
                .or_else(|| {
                    request
                        .headers()
                        .get(header::HOST)
                        .and_then(|value| value.to_str().ok())
                })
                .unwrap_or_default();

            if !host.is_empty() {
                let site_domain =
                    state
                        .cache
                        .site_domain(&state.db)
                        .await
                        .unwrap_or_else(|error| {
                            tracing::warn!(
                                error = %error,
                                "failed to load site_domain for the CSRF check"
                            );
                            String::new()
                        });

                let origin = normalize_authority(origin);
                let same_host = origin.is_some() && origin == normalize_authority(host);
                let configured_domain = normalize_authority(&site_domain);
                let same_domain =
                    origin.is_some() && configured_domain.is_some() && origin == configured_domain;

                if !same_host && !same_domain {
                    return Err(AppError::BadRequest(
                        "cross-origin request rejected".to_string(),
                    ));
                }
            }
        }
    }

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::normalize_authority;

    #[test]
    fn origin_comparison_ignores_scheme_and_default_ports() {
        assert_eq!(
            normalize_authority("https://api.example.com"),
            normalize_authority("api.example.com")
        );
        assert_eq!(
            normalize_authority("https://api.example.com:443"),
            normalize_authority("api.example.com")
        );
        assert_eq!(
            normalize_authority("http://localhost:5173"),
            normalize_authority("localhost:5173")
        );
        assert_ne!(
            normalize_authority("https://api.example.com"),
            normalize_authority("evil.example.com")
        );
    }
}
