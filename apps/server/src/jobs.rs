use crate::{AppError, AppState, uploads::UploadRow};
use aws_sdk_s3::{
    primitives::{ByteStream, Length},
    types::{CompletedMultipartUpload, CompletedPart},
};
use axum::{
    Json,
    extract::{Path, State, rejection::PathRejection},
    http::StatusCode,
};
use dagaz_protocol::{ApiError, AudioFormat, Job, JobKind, JobStatus, Quality, Track};
use lofty::{
    file::{AudioFile, FileType, TaggedFileExt},
    tag::{Accessor, ItemKey, TagType},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::PathBuf, sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct JobRow {
    id: String,
    kind: String,
    status: String,
    upload_id: Option<String>,
    candidate_track_id: Option<String>,
    result_json: Option<String>,
    errors_json: String,
    attempts: i64,
}
impl JobRow {
    fn dto(self) -> Result<Job, AppError> {
        let kind = match self.kind.as_str() {
            "import" => JobKind::Import,
            "retag" => JobKind::Retag,
            "transcode" => JobKind::Transcode,
            "cleanup" => JobKind::Cleanup,
            _ => return Err(AppError::internal("Invalid stored job kind")),
        };
        let status = match self.status.as_str() {
            "queued" => JobStatus::Queued,
            "running" => JobStatus::Running,
            "succeeded" => JobStatus::Succeeded,
            "failed" => JobStatus::Failed,
            "partial" => JobStatus::Partial,
            _ => return Err(AppError::internal("Invalid stored job state")),
        };
        Ok(Job {
            id: self
                .id
                .parse()
                .map_err(|_| AppError::internal("Invalid job ID"))?,
            kind,
            status,
            result: self
                .result_json
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            errors: serde_json::from_str(&self.errors_json)?,
        })
    }
}
pub(crate) async fn load(state: &AppState, id: &str) -> Result<Job, AppError> {
    sqlx::query_as::<_, JobRow>("SELECT * FROM jobs WHERE id=?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Job not found"))?
        .dto()
}
pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Job>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid job ID"))?;
    Ok(Json(load(&state, &id.to_string()).await?))
}

fn invalid_audio() -> AppError {
    AppError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_audio",
        "File is not a supported valid WAV, MP3 or FLAC",
    )
}
fn storage_unavailable() -> AppError {
    AppError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage_unavailable",
        "Storage is temporarily unavailable; job will retry",
    )
}

fn inspect(
    path: PathBuf,
    upload: UploadRow,
    id: Uuid,
) -> Result<(Track, Option<Vec<u8>>), AppError> {
    let mut file = std::fs::File::open(&path)?;
    if file.metadata()?.len() != upload.total_bytes as u64 {
        return Err(invalid_audio());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    if format!("{:x}", digest.finalize()) != upload.sha256 {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "checksum_mismatch",
            "File checksum does not match",
        ));
    }
    let audio = lofty::probe::Probe::open(&path)
        .map_err(|_| invalid_audio())?
        .guess_file_type()
        .map_err(|_| invalid_audio())?
        .read()
        .map_err(|_| invalid_audio())?;
    let format = match audio.file_type() {
        FileType::Wav => AudioFormat::Wav,
        FileType::Mpeg => AudioFormat::Mp3,
        FileType::Flac => AudioFormat::Flac,
        _ => return Err(invalid_audio()),
    };
    let tag = audio.tag(TagType::Id3v2).or_else(|| audio.primary_tag());
    let cover = tag
        .and_then(|tag| {
            tag.pictures()
                .iter()
                .find(|picture| picture.pic_type() == lofty::picture::PictureType::CoverFront)
        })
        .map(|picture| picture.data());
    if cover.is_some_and(|bytes| bytes.len() > crate::artworks::MAX_BYTES) {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_artwork",
            "Embedded artwork exceeds 10 MiB",
        ));
    }
    let cover = cover.map(<[u8]>::to_vec);
    let track = Track {
        id,
        revision: 1,
        filename: upload.filename,
        title: tag
            .and_then(|tag| tag.title())
            .map(|value| value.into_owned()),
        artist: tag
            .and_then(|tag| tag.artist())
            .map(|value| value.into_owned()),
        artist_id: None,
        album_id: None,
        album: tag
            .and_then(|tag| tag.album())
            .map(|value| value.into_owned()),
        album_artist: tag
            .and_then(|tag| tag.get_string(ItemKey::AlbumArtist))
            .map(str::to_owned),
        track_number: tag
            .and_then(|tag| tag.track())
            .and_then(|number| number.try_into().ok()),
        disc_number: tag
            .and_then(|tag| tag.disk())
            .and_then(|number| number.try_into().ok()),
        duration_ms: audio
            .properties()
            .duration()
            .as_millis()
            .try_into()
            .map_err(|_| invalid_audio())?,
        format,
        artwork_id: None,
        original_bytes: upload.total_bytes,
        ready_qualities: vec![Quality::Original],
    };
    crate::library::validate_track(&track)?;
    Ok((track, cover))
}

pub(crate) async fn publish_object(
    state: &AppState,
    job_id: &str,
    total_bytes: i64,
    sha256: &str,
    path: &std::path::Path,
    key: &str,
) -> Result<String, AppError> {
    let head = state
        .s3
        .head_object()
        .bucket(&state.config.s3_bucket)
        .key(key)
        .send()
        .await;
    match head {
        Ok(head) => {
            if head.content_length() != Some(total_bytes)
                || head
                    .metadata()
                    .and_then(|metadata| metadata.get("sha256"))
                    .map(String::as_str)
                    != Some(sha256)
            {
                return Err(AppError::internal(
                    "Stored object revision failed validation",
                ));
            }
            return head
                .e_tag()
                .map(str::to_owned)
                .ok_or_else(storage_unavailable);
        }
        Err(error)
            if error
                .raw_response()
                .is_some_and(|response| response.status().as_u16() == 404) => {}
        Err(_) => return Err(storage_unavailable()),
    }
    let mut multipart: Option<String> =
        sqlx::query_scalar("SELECT s3_upload_id FROM jobs WHERE id=?")
            .bind(job_id)
            .fetch_one(&state.db)
            .await?;
    let mut existing = BTreeMap::new();
    if let Some(id) = &multipart {
        let mut marker = None;
        loop {
            let page = state
                .s3
                .list_parts()
                .bucket(&state.config.s3_bucket)
                .key(key)
                .upload_id(id)
                .set_part_number_marker(marker)
                .send()
                .await;
            match page {
                Ok(page) => {
                    for part in page.parts() {
                        if let (Some(number), Some(etag), Some(size)) =
                            (part.part_number(), part.e_tag(), part.size())
                        {
                            existing.insert(number, (etag.to_owned(), size));
                        }
                    }
                    if page.is_truncated() != Some(true) {
                        break;
                    }
                    marker = Some(
                        page.next_part_number_marker()
                            .ok_or_else(storage_unavailable)?
                            .to_owned(),
                    );
                }
                Err(error)
                    if error
                        .raw_response()
                        .is_some_and(|response| response.status().as_u16() == 404) =>
                {
                    multipart = None;
                    existing.clear();
                    break;
                }
                Err(_) => return Err(storage_unavailable()),
            }
        }
    }
    let id = if let Some(id) = multipart {
        id
    } else {
        let created = state
            .s3
            .create_multipart_upload()
            .bucket(&state.config.s3_bucket)
            .key(key)
            .metadata("sha256", sha256)
            .send()
            .await
            .map_err(|_| storage_unavailable())?;
        let id = created
            .upload_id()
            .ok_or_else(storage_unavailable)?
            .to_owned();
        sqlx::query("UPDATE jobs SET s3_upload_id=? WHERE id=?")
            .bind(&id)
            .bind(job_id)
            .execute(&state.db)
            .await?;
        id
    };
    let mut offset = 0u64;
    let mut number = 1i32;
    let mut parts = Vec::new();
    while offset < total_bytes as u64 {
        let length = (total_bytes as u64 - offset).min(8 * 1024 * 1024);
        let etag = if let Some((etag, size)) = existing
            .get(&number)
            .filter(|(_, size)| *size == length as i64)
        {
            let _ = size;
            etag.clone()
        } else {
            let body = ByteStream::read_from()
                .path(path)
                .offset(offset)
                .length(Length::Exact(length))
                .build()
                .await
                .map_err(|_| AppError::internal("Cannot read staging file"))?;
            state
                .s3
                .upload_part()
                .bucket(&state.config.s3_bucket)
                .key(key)
                .upload_id(&id)
                .part_number(number)
                .content_length(length as i64)
                .body(body)
                .send()
                .await
                .map_err(|_| storage_unavailable())?
                .e_tag()
                .ok_or_else(storage_unavailable)?
                .to_owned()
        };
        parts.push(
            CompletedPart::builder()
                .part_number(number)
                .e_tag(etag)
                .build(),
        );
        offset += length;
        number += 1;
        sqlx::query("UPDATE jobs SET progress_bytes=? WHERE id=?")
            .bind(offset as i64)
            .bind(job_id)
            .execute(&state.db)
            .await?;
    }
    let result = state
        .s3
        .complete_multipart_upload()
        .bucket(&state.config.s3_bucket)
        .key(key)
        .upload_id(id)
        .multipart_upload(
            CompletedMultipartUpload::builder()
                .set_parts(Some(parts))
                .build(),
        )
        .send()
        .await
        .map_err(|_| storage_unavailable())?;
    result
        .e_tag()
        .map(str::to_owned)
        .ok_or_else(storage_unavailable)
}

async fn import(state: &AppState, job: &JobRow) -> Result<(), AppError> {
    let upload_id = job
        .upload_id
        .as_deref()
        .ok_or_else(|| AppError::internal("Import job has no upload"))?;
    let upload = crate::uploads::load(state, upload_id).await?;
    let path = state.config.data_dir.join("staging").join(upload_id);
    let id: Uuid = job
        .candidate_track_id
        .as_deref()
        .ok_or_else(|| AppError::internal("Import job has no track ID"))?
        .parse()
        .map_err(|_| AppError::internal("Invalid import track ID"))?;
    let sha256 = upload.sha256.clone();
    let bytes = upload.total_bytes;
    let source_path = path.clone();
    let (mut track, cover) = tokio::task::spawn_blocking(move || inspect(source_path, upload, id))
        .await
        .map_err(|_| AppError::internal("Audio inspection failed"))??;
    let duplicate:Option<String>=sqlx::query_scalar("SELECT r.track_id FROM media_revisions r JOIN tracks t ON t.id=r.track_id WHERE r.sha256=? AND t.deleted=0 LIMIT 1").bind(&sha256).fetch_optional(&state.db).await?;
    let final_id = if let Some(id) = duplicate {
        id
    } else {
        if let Some(cover) = cover {
            let mime = match image::guess_format(&cover)
                .map_err(|_| AppError::invalid("Invalid embedded artwork"))?
            {
                image::ImageFormat::Png => "image/png",
                image::ImageFormat::Jpeg => "image/jpeg",
                _ => return Err(AppError::invalid("Embedded artwork must be JPEG or PNG")),
            };
            track.artwork_id = Some(crate::artworks::store(state, id, mime, cover).await?);
        }
        let key = format!("tracks/{id}/1/original");
        let upload = crate::uploads::load(state, upload_id).await?;
        let storage_etag = publish_object(
            state,
            &job.id,
            upload.total_bytes,
            &upload.sha256,
            &path,
            &key,
        )
        .await?;
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        crate::library::write_track(&mut tx, &mut track).await?;
        sqlx::query("INSERT INTO media_revisions(track_id,revision,quality,object_key,byte_length,sha256,etag,storage_etag,created_at) VALUES(?,1,'original',?,?,?,?,?,?)").bind(id.to_string()).bind(&key).bind(bytes).bind(&sha256).bind(format!("\"{sha256}\"")).bind(storage_etag).bind(crate::unix_time()?).execute(&mut *tx).await?;
        finish(&mut tx, job, upload_id, &id.to_string()).await?;
        tx.commit().await?;
        let _ = tokio::fs::remove_file(&path).await;
        return Ok(());
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    finish(&mut tx, job, upload_id, &final_id).await?;
    tx.commit().await?;
    let _ = tokio::fs::remove_file(path).await;
    Ok(())
}
async fn finish(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job: &JobRow,
    upload: &str,
    track: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE jobs SET status='succeeded',result_json=?,errors_json='[]',s3_upload_id=NULL WHERE id=?").bind(serde_json::json!({"track_id":track}).to_string()).bind(&job.id).execute(&mut **tx).await?;
    sqlx::query("UPDATE uploads SET status='completed' WHERE id=?")
        .bind(upload)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn run_jobs(
    state: Arc<AppState>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(), AppError> {
    sqlx::query("UPDATE jobs SET status='queued' WHERE status='running'")
        .execute(&state.db)
        .await?;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let job:Option<JobRow>=sqlx::query_as("UPDATE jobs SET status='running',attempts=attempts+1 WHERE id=(SELECT id FROM jobs WHERE status='queued' AND not_before<=? ORDER BY rowid LIMIT 1) RETURNING *").bind(crate::unix_time()?).fetch_optional(&state.db).await?;
        if let Some(job) = job {
            let result = if job.kind == "import" {
                import(&state, &job).await
            } else if job.kind == "retag" {
                crate::tags::run(&state, &job.id).await
            } else {
                Err(AppError::internal("Unsupported job kind"))
            };
            if let Err(error) = result {
                if error.code == "database_error" {
                    return Err(error);
                }
                let errors = serde_json::to_string(&vec![ApiError {
                    code: error.code.into(),
                    message: error.message.into(),
                    request_id: Uuid::new_v4(),
                    job_id: Some(
                        job.id
                            .parse()
                            .map_err(|_| AppError::internal("Invalid job ID"))?,
                    ),
                }])?;
                let mut tx = state.db.begin().await?;
                if error.status == StatusCode::SERVICE_UNAVAILABLE {
                    let delay = 1i64
                        .checked_shl(job.attempts.min(6) as u32)
                        .unwrap_or(60)
                        .min(60);
                    sqlx::query(
                        "UPDATE jobs SET status='queued',errors_json=?,not_before=? WHERE id=?",
                    )
                    .bind(errors)
                    .bind(crate::unix_time()? + delay)
                    .bind(&job.id)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    sqlx::query("UPDATE jobs SET status='failed',errors_json=? WHERE id=?")
                        .bind(errors)
                        .bind(&job.id)
                        .execute(&mut *tx)
                        .await?;
                    if let Some(upload) = &job.upload_id {
                        sqlx::query("UPDATE uploads SET status='failed' WHERE id=?")
                            .bind(upload)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
                tx.commit().await?;
            }
        } else {
            tokio::select! {
                changed=shutdown.changed()=>{if changed.is_err(){return Ok(());}},
                _=state.job_notify.notified()=>{},
                _=tokio::time::sleep(Duration::from_millis(500))=>{},
            }
        }
    }
}
