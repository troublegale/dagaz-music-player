mod auth;
mod config;
mod jobs;
mod library;
mod media;
mod operations;
mod playlists;
mod sync;
pub use media::descriptor as media_descriptor;
mod uploads;
pub use jobs::run_jobs;

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
    job_notify: tokio::sync::Notify,
    media_slots: Arc<tokio::sync::Semaphore>,
    active_reads: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}
impl AppState {
    pub fn active_media_reads(&self) -> Result<usize, AppError> {
        self.active_reads
            .lock()
            .map(|counts| counts.values().sum())
            .map_err(|_| AppError::internal("Media read registry unavailable"))
    }
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
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            "Database operation failed",
        )
    }
}
impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::StorageFull {
            Self::new(
                StatusCode::INSUFFICIENT_STORAGE,
                "storage_full",
                "Insufficient server storage",
            )
        } else {
            Self::internal("Server filesystem operation failed")
        }
    }
}
impl From<serde_json::Error> for AppError {
    fn from(_: serde_json::Error) -> Self {
        Self::internal("Stored data serialization failed")
    }
}
fn unix_time() -> Result<i64, AppError> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| AppError::internal("System clock must be after Unix epoch"))?
        .as_secs() as i64)
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
    Ok(router(open_state(config, db, s3).await?))
}

pub async fn open_state(
    config: Config,
    db: SqlitePool,
    s3: aws_sdk_s3::Client,
) -> Result<Arc<AppState>, AppError> {
    config.validate()?;
    s3.head_bucket()
        .bucket(&config.s3_bucket)
        .send()
        .await
        .map_err(|_| {
            AppError::internal("S3 bucket check failed; check endpoint, bucket and credentials")
        })?;
    tokio::fs::create_dir_all(config.data_dir.join("staging")).await?;
    sync::initialize(&db).await?;
    library::backfill(&db).await?;
    Ok(Arc::new(AppState {
        db,
        s3,
        config,
        limits: auth::RateLimits::default(),
        job_notify: tokio::sync::Notify::new(),
        media_slots: Arc::new(tokio::sync::Semaphore::new(16)),
        active_reads: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    }))
}

pub fn router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/devices", get(auth::devices))
        .route("/devices/{id}", delete(auth::revoke))
        .route("/uploads", post(uploads::create))
        .route(
            "/uploads/{id}",
            get(uploads::get)
                .patch(uploads::patch)
                .layer(DefaultBodyLimit::max(uploads::MAX_CHUNK_BYTES))
                .route_layer(axum::middleware::from_fn(uploads::check_chunk_length)),
        )
        .route("/uploads/{id}/complete", post(uploads::complete))
        .route("/jobs/{id}", get(jobs::get))
        .route("/tracks", get(library::tracks))
        .route(
            "/tracks/{id}",
            get(library::track).delete(playlists::delete_track),
        )
        .route("/artists", get(library::artists))
        .route("/albums", get(library::albums))
        .route("/albums/{id}/tracks", get(library::album_tracks))
        .route("/snapshot", get(sync::snapshot))
        .route("/changes", get(sync::changes))
        .route("/playlists", get(playlists::list).post(playlists::create))
        .route(
            "/playlists/{id}",
            get(playlists::get)
                .patch(playlists::rename)
                .delete(playlists::delete),
        )
        .route(
            "/playlists/{id}/tracks",
            axum::routing::put(playlists::replace).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/tracks/{id}/media", get(media::stream).head(media::stream))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::authorize,
        ));
    Router::new()
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
        .with_state(state)
}
