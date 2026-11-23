use crate::{AppError, AppState};
use axum::{
    body::Body,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderMap, Method, Response, StatusCode, header},
    response::IntoResponse,
};
use dagaz_protocol::{AudioFormat, MediaDescriptor, Quality, Track};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(serde::Deserialize)]
pub(crate) struct MediaQuery {
    revision: i64,
    quality: Quality,
}
#[derive(sqlx::FromRow)]
struct Revision {
    object_key: String,
    byte_length: i64,
    sha256: String,
    etag: String,
    storage_etag: String,
    superseded_at: Option<i64>,
}
struct Media {
    revision: Revision,
    descriptor: MediaDescriptor,
    track: Track,
}
fn gone() -> AppError {
    AppError::new(
        StatusCode::GONE,
        "revision_gone",
        "Media revision is no longer available",
    )
}
fn unavailable() -> AppError {
    AppError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage_unavailable",
        "Media storage is unavailable or inconsistent",
    )
}

async fn resolve(
    state: &AppState,
    id: Uuid,
    revision: i64,
    quality: Quality,
) -> Result<Media, AppError> {
    if revision <= 0 {
        return Err(AppError::invalid("Revision must be positive"));
    }
    let current: Option<(String, i64)> =
        sqlx::query_as("SELECT data_json,deleted FROM tracks WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&state.db)
            .await?;
    let (payload, deleted) = current
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Track not found"))?;
    let pinned = crate::manifests::pinned(state, id, revision).await?;
    if deleted != 0 && !pinned {
        return Err(gone());
    }
    let original:Revision=sqlx::query_as("SELECT object_key,byte_length,sha256,etag,storage_etag,superseded_at FROM media_revisions WHERE track_id=? AND revision=? AND quality='original'").bind(id.to_string()).bind(revision).fetch_optional(&state.db).await?.ok_or_else(gone)?;
    if original
        .superseded_at
        .is_some_and(|at| crate::unix_time().is_ok_and(|now| now.saturating_sub(at) >= 86400))
        && !pinned
    {
        return Err(gone());
    }
    let effective = crate::qualities::effective(state, id, revision, quality).await?;
    let row = if effective == Quality::Original {
        original
    } else {
        sqlx::query_as::<_,Revision>("SELECT object_key,byte_length,sha256,etag,storage_etag,superseded_at FROM media_revisions WHERE track_id=? AND revision=? AND quality=?").bind(id.to_string()).bind(revision).bind(crate::qualities::name(effective)).fetch_optional(&state.db).await?.ok_or_else(unavailable)?
    };
    let now = crate::unix_time()?;
    if row
        .superseded_at
        .is_some_and(|timestamp| now.saturating_sub(timestamp) >= 86400)
        && !pinned
    {
        return Err(gone());
    }
    if row.byte_length <= 0 {
        return Err(unavailable());
    }
    let descriptor = MediaDescriptor {
        track_id: id,
        revision,
        requested_quality: quality,
        effective_quality: effective,
        byte_length: row.byte_length,
        sha256: row.sha256.clone(),
        etag: row.etag.clone(),
        url: format!(
            "/api/v1/tracks/{id}/media?revision={revision}&quality={}",
            crate::qualities::name(effective)
        ),
    };
    Ok(Media {
        revision: row,
        descriptor,
        track: serde_json::from_str(&payload)?,
    })
}
pub async fn descriptor(
    state: &AppState,
    id: Uuid,
    revision: i64,
    quality: Quality,
) -> Result<MediaDescriptor, AppError> {
    Ok(resolve(state, id, revision, quality).await?.descriptor)
}

fn byte_range(value: &str, length: u64) -> Option<(u64, u64)> {
    let (unit, numbers) = value.trim().split_once('=')?;
    if !unit.eq_ignore_ascii_case("bytes") || numbers.contains(',') {
        return None;
    }
    let (start, end) = numbers.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok().filter(|suffix| *suffix > 0)?;
        Some((length.saturating_sub(suffix), length - 1))
    } else {
        let start = start.parse::<u64>().ok().filter(|start| *start < length)?;
        let end = if end.is_empty() {
            length - 1
        } else {
            end.parse::<u64>().ok()?.min(length - 1)
        };
        (end >= start).then_some((start, end))
    }
}
fn invalid_range(length: u64) -> axum::response::Response {
    let mut response = AppError::new(
        StatusCode::RANGE_NOT_SATISFIABLE,
        "invalid_range",
        "Only a valid single byte range is supported",
    )
    .into_response();
    if let Ok(value) = format!("bytes */{length}").parse() {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    response
}

struct ReadLease {
    counts: Arc<Mutex<HashMap<String, usize>>>,
    key: String,
}
impl ReadLease {
    fn acquire(state: &AppState, key: &str) -> Result<Self, AppError> {
        *state
            .active_reads
            .lock()
            .map_err(|_| AppError::internal("Media read registry unavailable"))?
            .entry(key.into())
            .or_default() += 1;
        Ok(Self {
            counts: state.active_reads.clone(),
            key: key.into(),
        })
    }
}
impl Drop for ReadLease {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.counts.lock()
            && let Some(count) = counts.get_mut(&self.key)
        {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.key);
            }
        }
    }
}

pub(crate) async fn stream(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<MediaQuery>, QueryRejection>,
    method: Method,
    headers: HeaderMap,
) -> Result<axum::response::Response, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid track ID"))?;
    let Query(query) = query.map_err(|_| AppError::invalid("Revision and quality are required"))?;
    let media = resolve(&state, id, query.revision, query.quality).await?;
    let row = &media.revision;
    if let Some(condition) = headers.get(header::IF_MATCH)
        && !condition.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .any(|value| value.trim() == "*" || value.trim() == row.etag)
        })
    {
        return Err(AppError::new(
            StatusCode::PRECONDITION_FAILED,
            "etag_mismatch",
            "Media ETag does not match",
        ));
    }
    let length = row.byte_length as u64;
    let use_range = headers
        .get(header::IF_RANGE)
        .is_none_or(|condition| condition.to_str().ok() == Some(row.etag.as_str()));
    let selection = if use_range && headers.contains_key(header::RANGE) {
        if headers.get_all(header::RANGE).iter().count() != 1 {
            return Ok(invalid_range(length));
        }
        match headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| byte_range(value, length))
        {
            Some(bounds) => Some(bounds),
            None => return Ok(invalid_range(length)),
        }
    } else {
        None
    };
    let (start, end) = selection.unwrap_or((0, length - 1));
    let count = end - start + 1;
    let mime = if media.descriptor.effective_quality != Quality::Original {
        "audio/mpeg"
    } else {
        match media.track.format {
            AudioFormat::Wav => "audio/wav",
            AudioFormat::Mp3 => "audio/mpeg",
            AudioFormat::Flac => "audio/flac",
        }
    };
    let filename = media
        .track
        .filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("audio");
    let filename = if media.descriptor.effective_quality != Quality::Original {
        format!(
            "{}.mp3",
            filename
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(filename)
        )
    } else {
        filename.to_owned()
    };
    let filename =
        percent_encoding::utf8_percent_encode(&filename, percent_encoding::NON_ALPHANUMERIC);
    let mut response = Response::builder()
        .status(if selection.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, count)
        .header(header::ETAG, &row.etag)
        .header(header::ACCEPT_RANGES, "bytes")
        .header("X-Content-SHA256", &row.sha256)
        .header(
            "X-Effective-Quality",
            crate::qualities::name(media.descriptor.effective_quality),
        )
        .header(header::CONTENT_LOCATION, &media.descriptor.url)
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{id}\"; filename*=UTF-8''{filename}"),
        );
    if selection.is_some() {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{length}"),
        );
    }
    if method == Method::HEAD {
        let upstream = state
            .s3
            .head_object()
            .bucket(&state.config.s3_bucket)
            .key(&row.object_key)
            .if_match(&row.storage_etag)
            .send()
            .await
            .map_err(|error| {
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404)
                {
                    gone()
                } else {
                    unavailable()
                }
            })?;
        if upstream.content_length() != Some(row.byte_length)
            || upstream.e_tag() != Some(row.storage_etag.as_str())
        {
            return Err(unavailable());
        }
        return response
            .body(Body::empty())
            .map_err(|_| AppError::internal("Invalid media response headers"));
    }
    let permit = state.media_slots.clone().try_acquire_owned().map_err(|_| {
        AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "too_many_streams",
            "Media stream limit reached; retry later",
        )
    })?;
    let lease = ReadLease::acquire(&state, &row.object_key)?;
    let request = state
        .s3
        .get_object()
        .bucket(&state.config.s3_bucket)
        .key(&row.object_key)
        .if_match(&row.storage_etag);
    let request = if selection.is_some() {
        request.range(format!("bytes={start}-{end}"))
    } else {
        request
    };
    let upstream = request.send().await.map_err(|error| {
        if error
            .raw_response()
            .is_some_and(|response| response.status().as_u16() == 404)
        {
            gone()
        } else {
            unavailable()
        }
    })?;
    if upstream.content_length() != Some(count as i64)
        || upstream.e_tag() != Some(row.storage_etag.as_str())
    {
        return Err(unavailable());
    }
    if selection.is_some()
        && upstream.content_range() != Some(format!("bytes {start}-{end}/{length}").as_str())
    {
        return Err(unavailable());
    }
    let guards = (permit, lease);
    // Keep revision and stream permits until the response is consumed or cancelled.
    let bytes =
        tokio_util::io::ReaderStream::with_capacity(upstream.body.into_async_read(), 64 * 1024)
            .map(move |chunk| {
                let _keep_alive = &guards;
                chunk
            });
    response
        .body(Body::from_stream(bytes))
        .map_err(|_| AppError::internal("Invalid media response headers"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_boundaries_reject_overflow_and_multiple_ranges() {
        assert_eq!(byte_range("bytes=0-99", 10), Some((0, 9)));
        assert_eq!(byte_range("bytes=-99", 10), Some((0, 9)));
        for value in [
            "bytes=10-",
            "bytes=5-2",
            "bytes=-0",
            "bytes=0-1,4-5",
            "bytes=-",
            "bytes=999999999999999999999999-",
        ] {
            assert_eq!(byte_range(value, 10), None);
        }
    }
}
