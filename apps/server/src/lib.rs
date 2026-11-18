mod auth;
mod config;

pub use auth::new_pairing_code;
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
pub use config::{Config, create_s3, open_database};
use dagaz_protocol::ApiError;
use sqlx::SqlitePool;
use std::sync::Arc;

pub struct AppState {
    pub db: SqlitePool,
    pub s3: aws_sdk_s3::Client,
    pub config: Config,
    limits: auth::RateLimits,
}

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}
impl AppError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
    fn internal(message: &'static str) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
    fn invalid(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid or revoked credentials",
        )
    }
    fn rate_limited() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Request limit exceeded; retry later",
        )
    }
}
impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for AppError {}
impl From<sqlx::Error> for AppError {
    fn from(_: sqlx::Error) -> Self {
        Self::internal("Database operation failed")
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ApiError {
                code: self.code.into(),
                message: self.message.into(),
                request_id: uuid::Uuid::new_v4(),
                job_id: None,
            }),
        )
            .into_response();
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, header::HeaderValue::from_static("60"));
        }
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }
        response
    }
}

pub async fn build_app(
    config: Config,
    db: SqlitePool,
    s3: aws_sdk_s3::Client,
) -> Result<Router, AppError> {
    config.validate()?;
    s3.head_bucket()
        .bucket(&config.s3_bucket)
        .send()
        .await
        .map_err(|_| {
            AppError::internal("S3 bucket check failed; check endpoint, bucket and credentials")
        })?;
    let state = Arc::new(AppState {
        db,
        s3,
        config,
        limits: auth::RateLimits::default(),
    });
    let protected = Router::new()
        .route("/devices", get(auth::devices))
        .route("/devices/{id}", delete(auth::revoke))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::authorize,
        ));
    Ok(Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .nest(
            "/api/v1",
            Router::new()
                .route("/pair", post(auth::pair))
                .merge(protected),
        )
        .fallback(|| async {
            AppError::new(StatusCode::NOT_FOUND, "not_found", "Endpoint not found")
        })
        .method_not_allowed_fallback(|| async {
            AppError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "Method not allowed",
            )
        })
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state))
}
