use crate::{
    AppError, AppState,
    auth::Identity,
    operations::{key, recall, remember},
};
use axum::{
    Extension, Json,
    extract::{
        Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
};
use dagaz_protocol::{
    ApiError, Job, MetadataBatch, MetadataEdit, MetadataPatch, Patch, Quality, Track, TrackRevision,
};
use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    picture::{Picture, PictureType},
    tag::{Accessor, ItemKey, Tag},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Read,
    path::{Path as FilePath, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

fn conflict() -> AppError {
    AppError::new(
        StatusCode::CONFLICT,
        "revision_conflict",
        "Track changed; refresh before editing",
    )
}
fn invalid() -> AppError {
    AppError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_metadata",
        "Metadata could not be saved without changing the audio format",
    )
}
fn missing() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", "Track not found")
}
fn validate_patch(patch: &MetadataPatch) -> Result<(), AppError> {
    for field in [
        &patch.title,
        &patch.artist,
        &patch.album,
        &patch.album_artist,
    ] {
        if let Patch::Set(value) = field
            && (value.trim().is_empty()
                || value.len() > 4096
                || value.chars().any(|c| c.is_control() && !c.is_whitespace()))
        {
            return Err(AppError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_metadata",
                "Text must be nonempty, up to 4 KiB, without invalid controls; use null to clear",
            ));
        }
    }
    Ok(())
}
pub(crate) async fn edit(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<MetadataEdit>, JsonRejection>,
) -> Result<(StatusCode, Json<Job>), AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid track ID"))?;
    let Json(edit) = body.map_err(|_| AppError::invalid("Invalid metadata edit"))?;
    submit(
        &state,
        device.0,
        headers,
        MetadataBatch {
            items: vec![TrackRevision {
                track_id: id,
                expected_revision: edit.expected_revision,
            }],
            patch: edit.patch,
        },
        true,
    )
    .await
}
pub(crate) async fn batch(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    headers: HeaderMap,
    body: Result<Json<MetadataBatch>, JsonRejection>,
) -> Result<(StatusCode, Json<Job>), AppError> {
    let Json(batch) = body.map_err(|_| AppError::invalid("Invalid metadata batch"))?;
    submit(&state, device.0, headers, batch, false).await
}
async fn submit(
    state: &AppState,
    device: Uuid,
    headers: HeaderMap,
    batch: MetadataBatch,
    single: bool,
) -> Result<(StatusCode, Json<Job>), AppError> {
    let key = key(&headers)?;
    validate_patch(&batch.patch)?;
    let mut ids = HashSet::new();
    if batch.items.is_empty()
        || batch.items.len() > 100
        || batch
            .items
            .iter()
            .any(|item| item.expected_revision == i64::MAX || !ids.insert(item.track_id))
    {
        return Err(AppError::invalid(
            "Batch requires 1 to 100 distinct tracks with valid revisions",
        ));
    }
    let route = if single {
        format!("metadata:{}", batch.items[0].track_id)
    } else {
        "metadata_batch".into()
    };
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&batch)?));
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(id) = recall(&mut tx, device, &route, &key, &fingerprint).await? {
        tx.commit().await?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(crate::jobs::load(state, &id).await?),
        ));
    }
    if let Patch::Set(id) = batch.patch.artwork_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artworks WHERE id=?)")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if !exists {
            return Err(AppError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Artwork not found",
            ));
        }
    }
    if single {
        let item = &batch.items[0];
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM tracks WHERE id=? AND deleted=0")
                .bind(item.track_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if revision.ok_or_else(missing)? != item.expected_revision {
            return Err(conflict());
        }
    }
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO jobs(id,kind) VALUES(?,'retag')")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO metadata_jobs(job_id,patch_json) VALUES(?,?)")
        .bind(&id)
        .bind(serde_json::to_string(&batch.patch)?)
        .execute(&mut *tx)
        .await?;
    for (position, item) in batch.items.iter().enumerate() {
        sqlx::query("INSERT INTO metadata_items(job_id,position,track_id,expected_revision) VALUES(?,?,?,?)").bind(&id).bind(position as i64).bind(item.track_id.to_string()).bind(item.expected_revision).execute(&mut *tx).await?;
    }
    remember(&mut tx, device, &route, &key, &fingerprint, &id).await?;
    tx.commit().await?;
    state.job_notify.notify_one();
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::jobs::load(state, &id).await?),
    ))
}
fn merge<T: Clone>(value: &mut Option<T>, patch: &Patch<T>) {
    match patch {
        Patch::Missing => {}
        Patch::Clear => *value = None,
        Patch::Set(new) => *value = Some(new.clone()),
    }
}
fn apply(track: &mut Track, patch: &MetadataPatch) {
    merge(&mut track.title, &patch.title);
    merge(&mut track.artist, &patch.artist);
    merge(&mut track.album, &patch.album);
    merge(&mut track.album_artist, &patch.album_artist);
    merge(&mut track.track_number, &patch.track_number);
    merge(&mut track.disc_number, &patch.disc_number);
    merge(&mut track.artwork_id, &patch.artwork_id);
}
fn tag_field<T: ToString>(tag: &mut Tag, key: ItemKey, patch: &Patch<T>) {
    match patch {
        Patch::Missing => {}
        Patch::Clear => tag.remove_key(key),
        Patch::Set(value) => {
            tag.insert_text(key, value.to_string());
        }
    }
}
fn file_hash(path: &FilePath) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn rewrite(
    path: PathBuf,
    track: &Track,
    patch: &MetadataPatch,
    cover: Option<Vec<u8>>,
) -> Result<(i64, String), AppError> {
    let mut audio = lofty::probe::Probe::open(&path)
        .map_err(|_| invalid())?
        .guess_file_type()
        .map_err(|_| invalid())?
        .read()
        .map_err(|_| invalid())?;
    let expected_format = match track.format {
        dagaz_protocol::AudioFormat::Wav => FileType::Wav,
        dagaz_protocol::AudioFormat::Mp3 => FileType::Mpeg,
        dagaz_protocol::AudioFormat::Flac => FileType::Flac,
    };
    if audio.file_type() != expected_format {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_metadata",
            "Source container does not match catalog format",
        ));
    }
    let duration = audio.properties().duration();
    use lofty::tag::TagType::{Id3v2, RiffInfo, VorbisComments};
    let kinds: &[_] = match expected_format {
        FileType::Wav => &[RiffInfo, Id3v2],
        FileType::Mpeg => &[Id3v2],
        FileType::Flac => &[VorbisComments],
        _ => return Err(invalid()),
    };
    for &kind in kinds {
        if audio.tag(kind).is_none() {
            let mut tag = audio
                .primary_tag()
                .cloned()
                .unwrap_or_else(|| Tag::new(kind));
            tag.re_map(kind);
            audio.insert_tag(tag);
        }
        let tag = audio.tag_mut(kind).ok_or_else(invalid)?;
        tag_field(tag, ItemKey::TrackTitle, &patch.title);
        tag_field(tag, ItemKey::TrackArtist, &patch.artist);
        tag_field(tag, ItemKey::AlbumTitle, &patch.album);
        tag_field(tag, ItemKey::AlbumArtist, &patch.album_artist);
        tag_field(tag, ItemKey::TrackNumber, &patch.track_number);
        tag_field(tag, ItemKey::DiscNumber, &patch.disc_number);
        if kind != RiffInfo && !patch.artwork_id.is_missing() {
            tag.remove_picture_type(PictureType::CoverFront);
            if let Some(bytes) = &cover {
                let mut picture = Picture::from_reader(&mut std::io::Cursor::new(bytes))
                    .map_err(|_| invalid())?;
                picture.set_pic_type(PictureType::CoverFront);
                tag.push_picture(picture);
            }
        }
    }
    audio
        .save_to_path(&path, WriteOptions::default())
        .map_err(|_| invalid())?;
    let verified = lofty::probe::Probe::open(&path)
        .map_err(|_| invalid())?
        .guess_file_type()
        .map_err(|_| invalid())?
        .read()
        .map_err(|_| invalid())?;
    if verified.file_type() != expected_format || verified.properties().duration() != duration {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_metadata",
            "Saved audio format or duration differs from source",
        ));
    }
    let tag = verified
        .tag(if expected_format == FileType::Flac {
            VorbisComments
        } else {
            Id3v2
        })
        .ok_or_else(invalid)?;
    if tag.title().as_deref() != track.title.as_deref()
        || tag.artist().as_deref() != track.artist.as_deref()
        || tag.album().as_deref() != track.album.as_deref()
        || tag.get_string(ItemKey::AlbumArtist) != track.album_artist.as_deref()
        || tag.track() != track.track_number.map(u32::from)
        || tag.disk() != track.disc_number.map(u32::from)
    {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_metadata",
            "Saved metadata differs from requested fields",
        ));
    }
    if !patch.artwork_id.is_missing() {
        let picture = tag
            .pictures()
            .iter()
            .find(|picture| picture.pic_type() == PictureType::CoverFront);
        if picture.map(|picture| picture.data()) != cover.as_deref() {
            return Err(invalid());
        }
    }
    let length = std::fs::metadata(&path)?
        .len()
        .try_into()
        .map_err(|_| invalid())?;
    Ok((length, file_hash(&path)?))
}
#[derive(sqlx::FromRow)]
struct Item {
    position: i64,
    track_id: String,
    expected_revision: i64,
}
async fn retag(
    state: &AppState,
    job: &str,
    item: &Item,
    patch: &MetadataPatch,
) -> Result<(), AppError> {
    let payload: Option<String> =
        sqlx::query_scalar("SELECT data_json FROM tracks WHERE id=? AND deleted=0")
            .bind(&item.track_id)
            .fetch_optional(&state.db)
            .await?;
    let mut track: Track = serde_json::from_str(&payload.ok_or_else(missing)?)?;
    if track.revision != item.expected_revision {
        return Err(conflict());
    }
    let (key,length,sha,storage_etag):(String,i64,String,String)=sqlx::query_as("SELECT object_key,byte_length,sha256,storage_etag FROM media_revisions WHERE track_id=? AND revision=? AND quality='original'").bind(&item.track_id).bind(item.expected_revision).fetch_one(&state.db).await?;
    if length <= 0 || length as u64 > state.config.max_upload_bytes {
        return Err(invalid());
    }
    crate::uploads::check_space(
        state,
        (length as u64)
            .saturating_mul(2)
            .saturating_add(10 * 1024 * 1024),
    )?;
    let path = state
        .config
        .data_dir
        .join("staging")
        .join(format!("retag-{job}-{}", item.track_id));
    let result = retag_copy(
        state,
        job,
        item,
        patch,
        &mut track,
        &path,
        (&key, length, &sha, &storage_etag),
    )
    .await;
    let _ = tokio::fs::remove_file(path).await;
    result
}
async fn retag_copy(
    state: &AppState,
    job: &str,
    item: &Item,
    patch: &MetadataPatch,
    track: &mut Track,
    path: &FilePath,
    source: (&str, i64, &str, &str),
) -> Result<(), AppError> {
    let old_artist = track.artist_id.map(|id| id.to_string());
    let old_album = track.album_id.map(|id| id.to_string());
    let (key, length, sha, storage_etag) = source;
    let object = state
        .s3
        .get_object()
        .bucket(&state.config.s3_bucket)
        .key(key)
        .if_match(storage_etag)
        .send()
        .await
        .map_err(|_| crate::artworks::unavailable())?;
    if object.content_length() != Some(length) {
        return Err(crate::artworks::unavailable());
    }
    let mut output = tokio::fs::File::create(path).await?;
    let received = tokio::io::copy(
        &mut object.body.into_async_read().take(length as u64 + 1),
        &mut output,
    )
    .await
    .map_err(|_| crate::artworks::unavailable())?;
    output.flush().await?;
    output.sync_all().await?;
    drop(output);
    if received != length as u64 {
        return Err(crate::artworks::unavailable());
    }
    let copy = path.to_owned();
    let actual = tokio::task::spawn_blocking(move || file_hash(&copy))
        .await
        .map_err(|_| AppError::internal("Audio validation failed"))??;
    if actual != sha {
        return Err(crate::artworks::unavailable());
    }
    let cover = if let Patch::Set(id) = patch.artwork_id {
        Some(crate::artworks::original(state, id).await?)
    } else {
        None
    };
    apply(track, patch);
    crate::library::validate_track(track)?;
    track.revision = item.expected_revision.checked_add(1).ok_or_else(invalid)?;
    track.ready_qualities = vec![Quality::Original];
    let copy = path.to_owned();
    let desired = track.clone();
    let changes = patch.clone();
    let (bytes, hash) =
        tokio::task::spawn_blocking(move || rewrite(copy, &desired, &changes, cover))
            .await
            .map_err(|_| AppError::internal("Metadata writing failed"))??;
    if bytes <= 0 || bytes as u64 > state.config.max_upload_bytes {
        return Err(invalid());
    }
    track.original_bytes = bytes;
    // shortcut: orphan candidates remain until S8 grace-period cleanup; bound storage before deployment.
    let key = format!("tracks/{}/{}/original/{job}", item.track_id, track.revision);
    let storage_etag = crate::jobs::publish_object(state, job, bytes, &hash, path, &key).await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let current: Option<i64> =
        sqlx::query_scalar("SELECT revision FROM tracks WHERE id=? AND deleted=0")
            .bind(&item.track_id)
            .fetch_optional(&mut *tx)
            .await?;
    if current != Some(item.expected_revision) {
        return Err(conflict());
    }
    crate::library::write_track(&mut tx, track).await?;
    crate::library::remove_orphans(&mut tx, old_artist, old_album).await?;
    sqlx::query("UPDATE media_revisions SET superseded_at=? WHERE track_id=? AND revision=? AND superseded_at IS NULL").bind(crate::unix_time()?).bind(&item.track_id).bind(item.expected_revision).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO media_revisions(track_id,revision,quality,object_key,byte_length,sha256,etag,storage_etag,created_at) VALUES(?,?,'original',?,?,?,?,?,?)").bind(&item.track_id).bind(track.revision).bind(key).bind(bytes).bind(&hash).bind(format!("\"{hash}\"")).bind(storage_etag).bind(crate::unix_time()?).execute(&mut *tx).await?;
    let result = json!({"track_id":item.track_id,"status":"succeeded","revision":track.revision,"track":track});
    sqlx::query(
        "UPDATE metadata_items SET status='succeeded',result_json=? WHERE job_id=? AND position=?",
    )
    .bind(result.to_string())
    .bind(job)
    .bind(item.position)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE jobs SET s3_upload_id=NULL WHERE id=?")
        .bind(job)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn run(state: &AppState, job: &str) -> Result<(), AppError> {
    let patch: String = sqlx::query_scalar("SELECT patch_json FROM metadata_jobs WHERE job_id=?")
        .bind(job)
        .fetch_one(&state.db)
        .await?;
    let patch: MetadataPatch = serde_json::from_str(&patch)?;
    let items:Vec<Item>=sqlx::query_as("SELECT position,track_id,expected_revision FROM metadata_items WHERE job_id=? AND status='pending' ORDER BY position").bind(job).fetch_all(&state.db).await?;
    for item in items {
        if let Err(error) = retag(state, job, &item, &patch).await {
            if error.status == StatusCode::SERVICE_UNAVAILABLE || error.code == "database_error" {
                return Err(error);
            }
            let api = ApiError {
                code: error.code.into(),
                message: error.message.into(),
                request_id: Uuid::new_v4(),
                job_id: Some(
                    job.parse()
                        .map_err(|_| AppError::internal("Invalid job ID"))?,
                ),
            };
            let result = json!({"track_id":item.track_id,"status":"failed","error":api});
            let mut tx = state.db.begin().await?;
            sqlx::query("UPDATE metadata_items SET status='failed',result_json=? WHERE job_id=? AND position=?").bind(result.to_string()).bind(job).bind(item.position).execute(&mut *tx).await?;
            sqlx::query("UPDATE jobs SET s3_upload_id=NULL WHERE id=?")
                .bind(job)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
    }
    let results: Vec<String> = sqlx::query_scalar(
        "SELECT result_json FROM metadata_items WHERE job_id=? ORDER BY position",
    )
    .bind(job)
    .fetch_all(&state.db)
    .await?;
    let results: Vec<Value> = results
        .into_iter()
        .map(|json| serde_json::from_str(&json))
        .collect::<Result<_, _>>()?;
    let errors: Vec<_> = results
        .iter()
        .filter_map(|item| item.get("error").cloned())
        .collect();
    let status = if errors.is_empty() {
        "succeeded"
    } else if errors.len() == results.len() {
        "failed"
    } else {
        "partial"
    };
    let mut result = json!({"items":results});
    if let Some(first) = result["items"]
        .as_array()
        .filter(|items| items.len() == 1)
        .map(|items| items[0].clone())
    {
        for key in ["track_id", "revision", "track"] {
            if let Some(value) = first.get(key) {
                result[key] = value.clone();
            }
        }
    }
    sqlx::query(
        "UPDATE jobs SET status=?,result_json=?,errors_json=?,s3_upload_id=NULL WHERE id=?",
    )
    .bind(status)
    .bind(result.to_string())
    .bind(serde_json::to_string(&errors)?)
    .bind(job)
    .execute(&state.db)
    .await?;
    Ok(())
}
