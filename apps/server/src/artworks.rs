use crate::{AppError, AppState};
use aws_sdk_s3::primitives::ByteStream;
use axum::{
    Json,
    body::{Body, Bytes},
    extract::{
        Path, Query, State,
        rejection::{BytesRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use dagaz_protocol::ArtworkCreated;
use image::{ImageFormat, ImageReader};
use sha2::{Digest, Sha256};
use std::{io::Cursor, sync::Arc};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

pub(crate) const MAX_BYTES: usize = 10 * 1024 * 1024;
pub(crate) fn unavailable() -> AppError {
    AppError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage_unavailable",
        "Media storage is temporarily unavailable",
    )
}
fn invalid() -> AppError {
    AppError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_artwork",
        "Artwork must be a valid JPEG/PNG up to 25 megapixels",
    )
}
fn thumbnail(bytes: &[u8], mime: &str) -> Result<Vec<u8>, AppError> {
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        _ => {
            return Err(AppError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid_artwork_type",
                "Only JPEG and PNG are supported",
            ));
        }
    };
    if image::guess_format(bytes).map_err(|_| invalid())? != format {
        return Err(invalid());
    }
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| invalid())?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 25_000_000 {
        return Err(invalid());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| invalid())?;
    let mut output = Cursor::new(Vec::new());
    decoded
        .thumbnail(256, 256)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| invalid())?;
    Ok(output.into_inner())
}
pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<ArtworkCreated>), AppError> {
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = body.map_err(|_| {
        AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "artwork_too_large",
            "Artwork exceeds 10 MiB",
        )
    })?;
    let _slot = state
        .artwork_slots
        .try_acquire()
        .map_err(|_| AppError::rate_limited())?;
    let id = store(&state, Uuid::new_v4(), &mime, bytes.to_vec()).await?;
    Ok((StatusCode::CREATED, Json(ArtworkCreated { id })))
}
pub(crate) async fn store(
    state: &AppState,
    id: Uuid,
    mime: &str,
    bytes: Vec<u8>,
) -> Result<Uuid, AppError> {
    if bytes.len() > MAX_BYTES {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "artwork_too_large",
            "Artwork exceeds 10 MiB",
        ));
    }
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let stored: Option<(String, String)> =
        sqlx::query_as("SELECT sha256,mime FROM artworks WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&state.db)
            .await?;
    if let Some((saved, kind)) = stored {
        if saved != sha || kind != mime {
            return Err(unavailable());
        }
        return Ok(id);
    }
    let copy = bytes.clone();
    let kind = mime.to_owned();
    let small = tokio::task::spawn_blocking(move || thumbnail(&copy, &kind))
        .await
        .map_err(|_| AppError::internal("Artwork processing failed"))??;
    let thumb_sha = format!("{:x}", Sha256::digest(&small));
    for (key, data, hash, kind) in [
        (format!("artworks/{id}/original"), &bytes, &sha, mime),
        (
            format!("artworks/{id}/256"),
            &small,
            &thumb_sha,
            "image/png",
        ),
    ] {
        state
            .s3
            .put_object()
            .bucket(&state.config.s3_bucket)
            .key(key)
            .content_type(kind)
            .metadata("sha256", hash)
            .body(ByteStream::from(data.clone()))
            .send()
            .await
            .map_err(|_| unavailable())?;
    }
    sqlx::query("INSERT INTO artworks VALUES(?,?,?,?,?,?)")
        .bind(id.to_string())
        .bind(mime)
        .bind(sha)
        .bind(bytes.len() as i64)
        .bind(thumb_sha)
        .bind(small.len() as i64)
        .execute(&state.db)
        .await?;
    Ok(id)
}
pub(crate) async fn read_object(
    state: &AppState,
    key: &str,
    length: i64,
    hash: &str,
) -> Result<Vec<u8>, AppError> {
    if !(0..=MAX_BYTES as i64).contains(&length) {
        return Err(unavailable());
    }
    let object = state
        .s3
        .get_object()
        .bucket(&state.config.s3_bucket)
        .key(key)
        .send()
        .await
        .map_err(|_| unavailable())?;
    if object.content_length() != Some(length) {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    object
        .body
        .into_async_read()
        .take(length as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| unavailable())?;
    if bytes.len() as i64 != length || format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err(unavailable());
    }
    Ok(bytes)
}
pub(crate) async fn original(state: &AppState, id: Uuid) -> Result<Vec<u8>, AppError> {
    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT byte_length,sha256 FROM artworks WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&state.db)
            .await?;
    let (length, sha) =
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Artwork not found"))?;
    read_object(state, &format!("artworks/{id}/original"), length, &sha).await
}
#[derive(serde::Deserialize)]
pub(crate) struct ArtworkQuery {
    size: Option<u16>,
}
pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ArtworkQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid artwork ID"))?;
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid thumbnail size"))?;
    if query.size.is_some_and(|size| size != 256) {
        return Err(AppError::invalid("Supported thumbnail size is 256"));
    }
    let row: Option<(String, i64, String)> = sqlx::query_as(if query.size.is_some() {
        "SELECT 'image/png',thumbnail_length,thumbnail_sha256 FROM artworks WHERE id=?"
    } else {
        "SELECT mime,byte_length,sha256 FROM artworks WHERE id=?"
    })
    .bind(id.to_string())
    .fetch_optional(&state.db)
    .await?;
    let (mime, length, sha) =
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Artwork not found"))?;
    let suffix = if query.size.is_some() {
        "256"
    } else {
        "original"
    };
    let bytes = read_object(&state, &format!("artworks/{id}/{suffix}"), length, &sha).await?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_LENGTH, length.to_string()),
            (header::ETAG, format!("\"{sha}\"")),
            (header::CACHE_CONTROL, "private, max-age=86400".into()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
        ],
        Body::from(bytes),
    )
        .into_response())
}
