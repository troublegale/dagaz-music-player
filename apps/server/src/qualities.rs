use crate::{AppError, AppState};
use axum::http::StatusCode;
use dagaz_protocol::{Quality, Track};
use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    tag::{TagExt, TagType},
};
use sqlx::{Sqlite, Transaction};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};
use uuid::Uuid;

pub(crate) fn name(quality: Quality) -> &'static str {
    match quality {
        Quality::Original => "original",
        Quality::High => "high",
        Quality::Low => "low",
    }
}
pub(crate) async fn ensure(
    tx: &mut Transaction<'_, Sqlite>,
    track: Uuid,
    revision: i64,
    quality: Quality,
) -> Result<Uuid, AppError> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT job_id FROM quality_jobs WHERE track_id=? AND revision=? AND profile=?",
    )
    .bind(track.to_string())
    .bind(revision)
    .bind(name(quality))
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = stored {
        return id
            .parse()
            .map_err(|_| AppError::internal("Invalid quality job ID"));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO jobs(id,kind) VALUES(?,'transcode')")
        .bind(id.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO quality_jobs(job_id,track_id,revision,profile) VALUES(?,?,?,?)")
        .bind(id.to_string())
        .bind(track.to_string())
        .bind(revision)
        .bind(name(quality))
        .execute(&mut **tx)
        .await?;
    Ok(id)
}
pub(crate) async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    track: Uuid,
    revision: i64,
) -> Result<(), AppError> {
    for quality in [Quality::High, Quality::Low] {
        ensure(tx, track, revision, quality).await?;
    }
    Ok(())
}
pub(crate) async fn effective(
    state: &AppState,
    id: Uuid,
    revision: i64,
    quality: Quality,
) -> Result<Quality, AppError> {
    if quality == Quality::Original {
        return Ok(quality);
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let job = ensure(&mut tx, id, revision, quality).await?;
    tx.commit().await?;
    state.job_notify.notify_one();
    let (status,effective):(String,Option<String>)=sqlx::query_as("SELECT j.status,q.effective_quality FROM quality_jobs q JOIN jobs j ON j.id=q.job_id WHERE q.job_id=?").bind(job.to_string()).fetch_one(&state.db).await?;
    if status == "succeeded" {
        return serde_json::from_value(serde_json::Value::String(
            effective.ok_or_else(|| AppError::internal("Ready quality has no representation"))?,
        ))
        .map_err(AppError::from);
    }
    if status == "failed" {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "quality_failed",
            "Requested quality could not be generated; original remains available",
        )
        .with_job(job));
    }
    Err(AppError::new(
        StatusCode::TOO_EARLY,
        "representation_not_ready",
        "Requested quality is being prepared",
    )
    .with_job(job))
}
async fn command(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, AppError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| {
        AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "media_tool_unavailable",
            "Cannot start FFmpeg/FFprobe; check server installation",
        )
    })?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| AppError::internal("Media tool output unavailable"))?;
    let reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        output
            .take(65537)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map_err(|_| AppError::internal("Media tool wait failed"))?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = reader.await;
            return Err(AppError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "media_timeout",
                "Media conversion exceeded its time limit",
            ));
        }
    };
    let output = reader
        .await
        .map_err(|_| AppError::internal("Media tool reader failed"))??;
    if !status.success() || output.len() > 65536 {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "media_conversion_failed",
            "Media tool could not process this source",
        ));
    }
    Ok(output)
}
fn inspect(path: &Path) -> Result<lofty::file::TaggedFile, AppError> {
    lofty::probe::Probe::open(path)
        .map_err(|_| invalid())?
        .guess_file_type()
        .map_err(|_| invalid())?
        .read()
        .map_err(|_| invalid())
}
fn invalid() -> AppError {
    AppError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_representation",
        "Generated representation failed format, duration or metadata validation",
    )
}
async fn encode(
    state: &AppState,
    source: &Path,
    output: &Path,
    quality: Quality,
) -> Result<Quality, AppError> {
    let input = source.to_owned();
    let audio = tokio::task::spawn_blocking(move || inspect(&input))
        .await
        .map_err(|_| invalid())??;
    let format = match audio.file_type() {
        FileType::Mpeg => "mp3",
        FileType::Wav => "wav",
        FileType::Flac => "flac",
        _ => return Err(invalid()),
    };
    let target = if quality == Quality::High { 320 } else { 128 };
    if audio.file_type() == FileType::Mpeg {
        let bytes = command(
            Command::new(&state.config.ffprobe_path)
                .args([
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-f",
                    "mp3",
                    "-select_streams",
                    "a:0",
                    "-show_entries",
                    "stream=bit_rate",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                ])
                .arg(source),
            state.config.media_timeout.min(Duration::from_secs(30)),
        )
        .await?;
        let bitrate = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(invalid)?;
        if bitrate.div_ceil(1000) <= target {
            return Ok(Quality::Original);
        }
    }
    let output_budget = audio
        .properties()
        .duration()
        .as_secs()
        .saturating_add(1)
        .saturating_mul(target * 125)
        .saturating_add(10 * 1024 * 1024)
        .saturating_mul(2);
    crate::uploads::check_space(state, output_budget)?;
    command(
        Command::new(&state.config.ffmpeg_path)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-n",
                "-protocol_whitelist",
                "file,pipe",
                "-f",
            ])
            .arg(format)
            .arg("-i")
            .arg(source)
            .args([
                "-map",
                "0:a:0",
                "-map_metadata",
                "-1",
                "-c:a",
                "libmp3lame",
                "-b:a",
            ])
            .arg(format!("{target}k"))
            .args(["-threads", "1", "-f", "mp3"])
            .arg(output),
        state.config.media_timeout,
    )
    .await?;
    let encoded = output.to_owned();
    tokio::task::spawn_blocking(move || {
        let tag = audio
            .tag(TagType::Id3v2)
            .or_else(|| audio.primary_tag())
            .cloned();
        if let Some(mut tag) = tag {
            tag.re_map(TagType::Id3v2);
            tag.save_to_path(&encoded, WriteOptions::default())
                .map_err(|_| invalid())?;
        }
        let checked = inspect(&encoded)?;
        if checked.file_type() != FileType::Mpeg
            || checked
                .properties()
                .duration()
                .abs_diff(audio.properties().duration())
                .as_millis()
                > 500
        {
            return Err(invalid());
        }
        Ok(())
    })
    .await
    .map_err(|_| invalid())??;
    Ok(quality)
}
pub(crate) async fn run(state: &AppState, job: &str) -> Result<(), AppError> {
    let (id, revision, profile, ready): (String, i64, String, Option<String>) = sqlx::query_as(
        "SELECT track_id,revision,profile,effective_quality FROM quality_jobs WHERE job_id=?",
    )
    .bind(job)
    .fetch_one(&state.db)
    .await?;
    if ready.is_some() {
        sqlx::query("UPDATE jobs SET status='succeeded',errors_json='[]' WHERE id=?")
            .bind(job)
            .execute(&state.db)
            .await?;
        return Ok(());
    }
    let id: Uuid = id
        .parse()
        .map_err(|_| AppError::internal("Invalid quality track"))?;
    let quality: Quality = serde_json::from_value(serde_json::Value::String(profile))?;
    let source = state
        .config
        .data_dir
        .join("staging")
        .join(format!("transcode-{job}.source"));
    let output = source.with_extension("mp3");
    let _ = tokio::fs::remove_file(&output).await;
    let result = convert(state, job, id, revision, quality, &source, &output).await;
    for path in [source, output] {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}
async fn convert(
    state: &AppState,
    job: &str,
    id: Uuid,
    revision: i64,
    quality: Quality,
    source: &Path,
    output: &Path,
) -> Result<(), AppError> {
    let pinned = crate::manifests::pinned(state, id, revision).await?;
    let live: bool = sqlx::query_scalar("SELECT deleted=0 FROM tracks WHERE id=?")
        .bind(id.to_string())
        .fetch_one(&state.db)
        .await?;
    if !live && !pinned {
        return Err(AppError::new(
            StatusCode::GONE,
            "revision_gone",
            "Source track was deleted",
        ));
    }
    let row:Option<(String,i64,String,String,Option<i64>)>=sqlx::query_as("SELECT object_key,byte_length,sha256,storage_etag,superseded_at FROM media_revisions WHERE track_id=? AND revision=? AND quality='original'").bind(id.to_string()).bind(revision).fetch_optional(&state.db).await?;
    let (key, length, sha, etag, superseded) = row.ok_or_else(|| {
        AppError::new(
            StatusCode::GONE,
            "revision_gone",
            "Source revision is no longer available",
        )
    })?;
    if length <= 0 || length as u64 > state.config.max_upload_bytes {
        return Err(invalid());
    }
    let now = crate::unix_time()?;
    if superseded.is_some_and(|at| now.saturating_sub(at) >= 86400) && !pinned {
        return Err(AppError::new(
            StatusCode::GONE,
            "revision_gone",
            "Source revision expired",
        ));
    }
    crate::uploads::check_space(state, (length as u64).saturating_mul(2))?;
    let data = state
        .s3
        .get_object()
        .bucket(&state.config.s3_bucket)
        .key(key)
        .if_match(etag)
        .send()
        .await
        .map_err(|_| crate::artworks::unavailable())?;
    if data.content_length() != Some(length) {
        return Err(crate::artworks::unavailable());
    }
    let mut file = tokio::fs::File::create(source).await?;
    let count = tokio::io::copy(
        &mut data.body.into_async_read().take(length as u64 + 1),
        &mut file,
    )
    .await
    .map_err(|_| crate::artworks::unavailable())?;
    file.sync_all().await?;
    drop(file);
    if count != length as u64 {
        return Err(crate::artworks::unavailable());
    }
    let path = source.to_owned();
    let hash = tokio::task::spawn_blocking(move || crate::tags::file_hash(&path))
        .await
        .map_err(|_| invalid())??;
    if hash != sha {
        return Err(crate::artworks::unavailable());
    }
    let effective = encode(state, source, output, quality).await?;
    let published = if effective != Quality::Original {
        let path = output.to_owned();
        let hash = tokio::task::spawn_blocking(move || crate::tags::file_hash(&path))
            .await
            .map_err(|_| invalid())??;
        let length = tokio::fs::metadata(output).await?.len();
        if length == 0 || length > state.config.max_upload_bytes {
            return Err(invalid());
        }
        let key = format!("tracks/{id}/{revision}/{}/{hash}", name(quality));
        let etag =
            crate::jobs::publish_object(state, job, length as i64, &hash, output, &key).await?;
        Some((key, length as i64, hash, etag))
    } else {
        None
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some((key, length, sha, etag)) = published {
        sqlx::query("INSERT INTO media_revisions(track_id,revision,quality,object_key,byte_length,sha256,etag,storage_etag,created_at,superseded_at) VALUES(?,?,?,?,?,?,?,?,?,?)").bind(id.to_string()).bind(revision).bind(name(quality)).bind(key).bind(length).bind(&sha).bind(format!("\"{sha}\"")).bind(etag).bind(crate::unix_time()?).bind(superseded).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE quality_jobs SET effective_quality=? WHERE job_id=?")
        .bind(name(effective))
        .bind(job)
        .execute(&mut *tx)
        .await?;
    let current: Option<String> =
        sqlx::query_scalar("SELECT data_json FROM tracks WHERE id=? AND revision=? AND deleted=0")
            .bind(id.to_string())
            .bind(revision)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(payload) = current {
        let mut track: Track = serde_json::from_str(&payload)?;
        if !track.ready_qualities.contains(&quality) {
            track.ready_qualities.push(quality);
            crate::library::write_track(&mut tx, &mut track).await?;
        }
    }
    sqlx::query("UPDATE jobs SET status='succeeded',s3_upload_id=NULL,errors_json='[]',result_json=? WHERE id=?").bind(serde_json::json!({"track_id":id,"revision":revision,"requested_quality":quality,"effective_quality":effective}).to_string()).bind(job).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
