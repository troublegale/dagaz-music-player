use crate::{AppError, AppState, auth::Identity};
use axum::{
    Extension, Json,
    body::Bytes,
    extract::{
        Path, State,
        rejection::{BytesRejection, JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
};
use dagaz_protocol::{Job, UploadCreate, UploadState, UploadStatus};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

pub(crate) const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
pub(crate) async fn check_chunk_length(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, AppError> {
    if request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_CHUNK_BYTES as u64)
    {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk_too_large",
            "Chunk exceeds 8 MiB",
        ));
    }
    Ok(next.run(request).await)
}

#[derive(sqlx::FromRow)]
pub(crate) struct UploadRow {
    pub id: String,
    pub filename: String,
    pub total_bytes: i64,
    pub sha256: String,
    pub offset: i64,
    pub status: String,
    pub expires_at: i64,
    pub job_id: Option<String>,
}
impl UploadRow {
    fn dto(&self) -> Result<UploadState, AppError> {
        let status = match self.status.as_str() {
            "uploading" if self.expires_at <= crate::unix_time()? => UploadStatus::Expired,
            "uploading" => UploadStatus::Uploading,
            "processing" => UploadStatus::Processing,
            "completed" => UploadStatus::Completed,
            "failed" => UploadStatus::Failed,
            _ => return Err(AppError::internal("Invalid upload state in database")),
        };
        Ok(UploadState {
            id: self
                .id
                .parse()
                .map_err(|_| AppError::internal("Invalid upload ID"))?,
            offset: self.offset,
            total_bytes: self.total_bytes,
            expires_at: time::OffsetDateTime::from_unix_timestamp(self.expires_at)
                .map_err(|_| AppError::internal("Invalid upload expiry"))?
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| AppError::internal("Invalid upload expiry"))?,
            status,
        })
    }
    fn writable(&self) -> Result<(), AppError> {
        if self.status != "uploading" {
            return Err(conflict("Upload is no longer writable"));
        }
        if self.expires_at <= crate::unix_time()? {
            return Err(AppError::new(
                StatusCode::GONE,
                "upload_expired",
                "Upload expired; start a new upload",
            ));
        }
        Ok(())
    }
}
fn conflict(message: &'static str) -> AppError {
    AppError::new(StatusCode::CONFLICT, "conflict", message)
}
fn key(headers: &HeaderMap) -> Result<String, AppError> {
    Ok(headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::invalid("Idempotency-Key is required"))?
        .parse::<Uuid>()
        .map_err(|_| AppError::invalid("Idempotency-Key must be a UUID"))?
        .to_string())
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(crate) fn check_space(state: &AppState, needed: u64) -> Result<(), AppError> {
    if fs2::available_space(&state.config.data_dir)?
        < state.config.min_free_bytes.saturating_add(needed)
    {
        return Err(AppError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "storage_full",
            "Insufficient server storage",
        ));
    }
    Ok(())
}
async fn recall(
    tx: &mut Transaction<'_, Sqlite>,
    device: Uuid,
    route: &str,
    key: &str,
    fingerprint: &str,
) -> Result<Option<String>, AppError> {
    let stored: Option<(String, String)> = sqlx::query_as(
        "SELECT fingerprint,result_id FROM idempotency WHERE device_id=? AND route=? AND key=?",
    )
    .bind(device.to_string())
    .bind(route)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?;
    match stored {
        Some((saved, id)) if saved == fingerprint => Ok(Some(id)),
        Some(_) => Err(conflict("Idempotency key was used for a different request")),
        None => Ok(None),
    }
}
async fn remember(
    tx: &mut Transaction<'_, Sqlite>,
    device: Uuid,
    route: &str,
    key: &str,
    fingerprint: &str,
    id: &str,
) -> Result<(), AppError> {
    sqlx::query("INSERT INTO idempotency(device_id,route,key,fingerprint,result_id,created_at) VALUES(?,?,?,?,?,?)").bind(device.to_string()).bind(route).bind(key).bind(fingerprint).bind(id).bind(crate::unix_time()?).execute(&mut **tx).await?;
    Ok(())
}
pub(crate) async fn load(state: &AppState, id: &str) -> Result<UploadRow, AppError> {
    sqlx::query_as("SELECT * FROM uploads WHERE id=?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Upload not found"))
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    headers: HeaderMap,
    body: Result<Json<UploadCreate>, JsonRejection>,
) -> Result<(StatusCode, Json<UploadState>), AppError> {
    let Json(mut request) = body.map_err(|_| AppError::invalid("Invalid upload request"))?;
    let key = key(&headers)?;
    if request.filename.trim().is_empty()
        || request.filename.len() > 1024
        || request.filename.chars().any(char::is_control)
        || !valid_hash(&request.sha256)
    {
        return Err(AppError::invalid("Invalid filename or SHA-256"));
    }
    if request.total_bytes as u64 > state.config.max_upload_bytes {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            "File exceeds the configured size limit",
        ));
    }
    request.sha256.make_ascii_lowercase();
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&request)?));
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(id) = recall(&mut tx, device.0, "create_upload", &key, &fingerprint).await? {
        tx.commit().await?;
        return Ok((StatusCode::CREATED, Json(load(&state, &id).await?.dto()?)));
    }
    check_space(&state, 0)?;
    let id = Uuid::new_v4().to_string();
    let staging = state.config.data_dir.join("staging");
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging.join(&id))
        .await?;
    file.sync_all().await?;
    #[cfg(unix)]
    tokio::fs::File::open(&staging).await?.sync_all().await?;
    let expires = crate::unix_time()? + 86400;
    sqlx::query("INSERT INTO uploads(id,filename,total_bytes,sha256,expires_at) VALUES(?,?,?,?,?)")
        .bind(&id)
        .bind(&request.filename)
        .bind(request.total_bytes)
        .bind(&request.sha256)
        .bind(expires)
        .execute(&mut *tx)
        .await?;
    remember(&mut tx, device.0, "create_upload", &key, &fingerprint, &id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(load(&state, &id).await?.dto()?)))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<UploadState>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid upload ID"))?;
    Ok(Json(load(&state, &id.to_string()).await?.dto()?))
}

pub(crate) async fn patch(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<UploadState>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid upload ID"))?;
    let id = id.to_string();
    let key = key(&headers)?;
    let body = body.map_err(|_| {
        AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk_too_large",
            "Chunk exceeds 8 MiB",
        )
    })?;
    if body.is_empty() {
        return Err(AppError::invalid("Chunk must not be empty"));
    }
    if headers
        .get("Content-Type")
        .and_then(|value| value.to_str().ok())
        != Some("application/octet-stream")
    {
        return Err(AppError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_content_type",
            "Binary chunk required",
        ));
    }
    let offset = headers
        .get("Upload-Offset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|offset| *offset >= 0)
        .ok_or_else(|| AppError::invalid("Invalid Upload-Offset"))?;
    let hash = headers
        .get("X-Chunk-SHA256")
        .and_then(|value| value.to_str().ok())
        .filter(|hash| valid_hash(hash))
        .ok_or_else(|| AppError::invalid("Invalid chunk SHA-256"))?
        .to_ascii_lowercase();
    if format!("{:x}", Sha256::digest(&body)) != hash {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "checksum_mismatch",
            "Chunk checksum does not match",
        ));
    }
    let route = format!("patch_upload/{id}");
    let fingerprint = format!("{offset}:{}:{hash}", body.len());
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if recall(&mut tx, device.0, &route, &key, &fingerprint)
        .await?
        .is_some()
    {
        tx.commit().await?;
        return Ok(Json(load(&state, &id).await?.dto()?));
    }
    let upload: UploadRow = sqlx::query_as("SELECT * FROM uploads WHERE id=?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Upload not found"))?;
    upload.writable()?;
    let end = offset
        .checked_add(body.len() as i64)
        .filter(|end| *end <= upload.total_bytes)
        .ok_or_else(|| conflict("Chunk exceeds declared file length"))?;
    if offset > upload.offset {
        return Err(conflict("Upload offset does not match"));
    }
    let path = state.config.data_dir.join("staging").join(&id);
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true).write(true);
    if upload.offset == 0 {
        options.create(true);
    }
    let mut file = options.open(path).await?;
    let actual = file.metadata().await?.len();
    if actual < upload.offset as u64 {
        return Err(AppError::internal(
            "Staging file is shorter than its committed offset",
        ));
    }
    file.set_len(upload.offset as u64).await?;
    file.seek(std::io::SeekFrom::Start(offset as u64)).await?;
    if offset < upload.offset {
        if end > upload.offset {
            return Err(conflict("Chunk overlaps the committed offset"));
        }
        let mut previous = vec![0; body.len()];
        file.read_exact(&mut previous).await?;
        if previous.as_slice() != body.as_ref() {
            return Err(conflict("Retried chunk differs from committed bytes"));
        }
    } else {
        check_space(&state, body.len() as u64)?;
        file.write_all(&body).await?;
        file.sync_all().await?;
        sqlx::query("UPDATE uploads SET offset=? WHERE id=?")
            .bind(end)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    remember(&mut tx, device.0, &route, &key, &fingerprint, &id).await?;
    tx.commit().await?;
    Ok(Json(load(&state, &id).await?.dto()?))
}

pub(crate) async fn complete(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Job>), AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid upload ID"))?;
    let id = id.to_string();
    let key = key(&headers)?;
    let route = format!("complete_upload/{id}");
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(job) = recall(&mut tx, device.0, &route, &key, "").await? {
        tx.commit().await?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(crate::jobs::load(&state, &job).await?),
        ));
    }
    let upload: UploadRow = sqlx::query_as("SELECT * FROM uploads WHERE id=?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Upload not found"))?;
    let job = if let Some(job) = upload.job_id {
        job
    } else {
        upload.writable()?;
        if upload.offset != upload.total_bytes {
            return Err(conflict("Upload is incomplete"));
        }
        let job = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO jobs(id,kind,upload_id,candidate_track_id) VALUES(?,'import',?,?)",
        )
        .bind(&job)
        .bind(&id)
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE uploads SET status='processing',job_id=? WHERE id=?")
            .bind(&job)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        job
    };
    remember(&mut tx, device.0, &route, &key, "", &job).await?;
    tx.commit().await?;
    state.job_notify.notify_one();
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::jobs::load(&state, &job).await?),
    ))
}
