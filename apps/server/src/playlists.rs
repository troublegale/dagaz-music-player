use crate::{AppError, AppState, auth::Identity, library, operations, sync};
use axum::{
    Extension, Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
};
use dagaz_protocol::{
    Entity, Page, Playlist, PlaylistCreate, PlaylistRename, PlaylistTracks, RevisionRequest,
};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};
use std::sync::Arc;
use uuid::Uuid;

fn missing() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", "Catalog item not found")
}
fn conflict() -> AppError {
    AppError::new(
        StatusCode::CONFLICT,
        "revision_conflict",
        "Catalog revision changed; reload before saving",
    )
}
fn name(value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(AppError::invalid(
            "Name must be nonempty, at most 4 KiB and contain no controls",
        ));
    }
    Ok(())
}
#[derive(serde::Serialize, serde::Deserialize)]
struct EntryCursor {
    id: Uuid,
    revision: i64,
    position: i64,
}
#[derive(serde::Deserialize, Default)]
pub(crate) struct EntryQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}
async fn load(tx: &mut Transaction<'_, Sqlite>, id: Uuid) -> Result<Playlist, AppError> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT data_json FROM playlists WHERE id=? AND deleted=0")
            .bind(id.to_string())
            .fetch_optional(&mut **tx)
            .await?;
    Ok(serde_json::from_str(&value.ok_or_else(missing)?)?)
}
async fn page(
    tx: &mut Transaction<'_, Sqlite>,
    mut playlist: Playlist,
    query: EntryQuery,
) -> Result<Playlist, AppError> {
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(AppError::invalid("Limit must be between 1 and 500"));
    }
    let after = if let Some(cursor) = query.cursor {
        let cursor: EntryCursor = library::decode(&cursor)?;
        if cursor.id != playlist.id || cursor.position < -1 {
            return Err(AppError::invalid("Invalid playlist cursor"));
        }
        if cursor.revision != playlist.revision {
            return Err(conflict());
        }
        cursor.position
    } else {
        -1
    };
    let rows:Vec<(i64,String)>=sqlx::query_as("SELECT position,track_id FROM playlist_entries WHERE playlist_id=? AND position>? ORDER BY position LIMIT ?").bind(playlist.id.to_string()).bind(after).bind(i64::from(limit)+1).fetch_all(&mut **tx).await?;
    let more = rows.len() > limit as usize;
    playlist.track_ids.clear();
    playlist.next_cursor = None;
    for (position, id) in rows.into_iter().take(limit as usize) {
        playlist.track_ids.push(
            id.parse()
                .map_err(|_| AppError::internal("Invalid stored track ID"))?,
        );
        if more {
            playlist.next_cursor = Some(library::encode(&EntryCursor {
                id: playlist.id,
                revision: playlist.revision,
                position,
            })?);
        }
    }
    Ok(playlist)
}
async fn publish(
    tx: &mut Transaction<'_, Sqlite>,
    playlist: &mut Playlist,
) -> Result<(), AppError> {
    playlist.track_ids.clear();
    playlist.next_cursor = if playlist.total_count > 0 {
        Some(library::encode(&EntryCursor {
            id: playlist.id,
            revision: playlist.revision,
            position: -1,
        })?)
    } else {
        None
    };
    sqlx::query("UPDATE playlists SET name=?,normalized_name=?,revision=?,track_count=?,data_json=? WHERE id=?").bind(&playlist.name).bind(library::normal(&playlist.name)).bind(playlist.revision).bind(playlist.total_count).bind(serde_json::to_string(playlist)?).bind(playlist.id.to_string()).execute(&mut **tx).await?;
    sync::record(
        tx,
        Entity::Playlist,
        playlist.id,
        playlist.revision,
        Some(serde_json::to_value(playlist)?),
    )
    .await?;
    Ok(())
}
pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    query: Result<Query<library::ListQuery>, QueryRejection>,
) -> Result<Json<Page<Playlist>>, AppError> {
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid playlist query"))?;
    Ok(Json(library::named_page(&state, query, "playlists").await?))
}
pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<EntryQuery>, QueryRejection>,
) -> Result<Json<Playlist>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid playlist ID"))?;
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid playlist query"))?;
    let mut tx = state.db.begin().await?;
    let playlist = load(&mut tx, id).await?;
    let playlist = page(&mut tx, playlist, query).await?;
    tx.commit().await?;
    Ok(Json(playlist))
}
#[derive(serde::Serialize)]
enum Action {
    Create(PlaylistCreate),
    Rename(PlaylistRename),
    Replace(PlaylistTracks),
    Delete(RevisionRequest),
    DeleteTrack(RevisionRequest),
}
impl Action {
    fn route(&self, id: Uuid) -> String {
        match self {
            Self::Create(_) => "create_playlist".into(),
            Self::Rename(_) => format!("rename_playlist:{id}"),
            Self::Replace(_) => format!("replace_playlist:{id}"),
            Self::Delete(_) => format!("delete_playlist:{id}"),
            Self::DeleteTrack(_) => format!("delete_track:{id}"),
        }
    }
}
async fn mutate(
    state: &AppState,
    device: Uuid,
    headers: HeaderMap,
    id: Uuid,
    action: Action,
) -> Result<Option<Playlist>, AppError> {
    let key = operations::key(&headers)?;
    let route = action.route(id);
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&action)?));
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(saved) = operations::recall(&mut tx, device, &route, &key, &fingerprint).await? {
        if matches!(action, Action::Delete(_) | Action::DeleteTrack(_)) {
            tx.commit().await?;
            return Ok(None);
        }
        let saved = saved
            .parse()
            .map_err(|_| AppError::internal("Invalid operation result ID"))?;
        let playlist = load(&mut tx, saved).await?;
        let result = page(&mut tx, playlist, EntryQuery::default()).await?;
        tx.commit().await?;
        return Ok(Some(result));
    }
    let mut result = None;
    match action {
        Action::Create(request) => {
            name(&request.name)?;
            let mut playlist = Playlist {
                id,
                revision: 1,
                name: request.name.trim().into(),
                track_ids: Vec::new(),
                next_cursor: None,
                total_count: 0,
            };
            sqlx::query(
                "INSERT INTO playlists(id,name,normalized_name,data_json) VALUES(?,?,?,'{}')",
            )
            .bind(id.to_string())
            .bind(&playlist.name)
            .bind(library::normal(&playlist.name))
            .execute(&mut *tx)
            .await?;
            publish(&mut tx, &mut playlist).await?;
            result = Some(playlist);
        }
        Action::Rename(request) => {
            name(&request.name)?;
            let mut playlist = load(&mut tx, id).await?;
            if playlist.revision != request.expected_revision {
                return Err(conflict());
            }
            playlist.name = request.name.trim().into();
            playlist.revision += 1;
            publish(&mut tx, &mut playlist).await?;
            result = Some(page(&mut tx, playlist, EntryQuery::default()).await?);
        }
        Action::Replace(request) => {
            let mut playlist = load(&mut tx, id).await?;
            if playlist.revision != request.expected_revision {
                return Err(conflict());
            }
            for chunk in request.track_ids.chunks(250) {
                let unique: std::collections::HashSet<_> = chunk.iter().copied().collect();
                let mut query = sqlx::QueryBuilder::<Sqlite>::new(
                    "SELECT COUNT(*) FROM tracks WHERE deleted=0 AND id IN (",
                );
                let mut values = query.separated(",");
                for id in &unique {
                    values.push_bind(id.to_string());
                }
                values.push_unseparated(")");
                let count: i64 = query.build_query_scalar().fetch_one(&mut *tx).await?;
                if count != unique.len() as i64 {
                    return Err(AppError::invalid("Playlist contains an unavailable track"));
                }
            }
            sqlx::query("DELETE FROM playlist_entries WHERE playlist_id=?")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            for (position, track) in request.track_ids.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO playlist_entries(playlist_id,position,track_id) VALUES(?,?,?)",
                )
                .bind(id.to_string())
                .bind(position as i64)
                .bind(track.to_string())
                .execute(&mut *tx)
                .await?;
            }
            playlist.total_count = request.track_ids.len() as i64;
            playlist.revision += 1;
            publish(&mut tx, &mut playlist).await?;
            result = Some(page(&mut tx, playlist, EntryQuery::default()).await?);
        }
        Action::Delete(request) => {
            let playlist = load(&mut tx, id).await?;
            if playlist.revision != request.expected_revision {
                return Err(conflict());
            }
            sqlx::query("DELETE FROM playlist_entries WHERE playlist_id=?")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE playlists SET deleted=1,revision=revision+1 WHERE id=?")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            sync::record(&mut tx, Entity::Playlist, id, playlist.revision + 1, None).await?;
        }
        Action::DeleteTrack(request) => {
            remove_track(&mut tx, id, request.expected_revision).await?;
        }
    }
    operations::remember(&mut tx, device, &route, &key, &fingerprint, &id.to_string()).await?;
    tx.commit().await?;
    Ok(result)
}
async fn remove_track(
    tx: &mut Transaction<'_, Sqlite>,
    id: Uuid,
    expected: i64,
) -> Result<(), AppError> {
    let row: Option<(i64, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT revision,artist_id,album_id FROM tracks WHERE id=? AND deleted=0")
            .bind(id.to_string())
            .fetch_optional(&mut **tx)
            .await?;
    let (revision, artist, album) = row.ok_or_else(missing)?;
    if revision != expected {
        return Err(conflict());
    }
    let playlists: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT playlist_id FROM playlist_entries WHERE track_id=?")
            .bind(id.to_string())
            .fetch_all(&mut **tx)
            .await?;
    sqlx::query("DELETE FROM playlist_entries WHERE track_id=?")
        .bind(id.to_string())
        .execute(&mut **tx)
        .await?;
    for playlist in playlists {
        let playlist_id = playlist
            .parse()
            .map_err(|_| AppError::internal("Invalid stored playlist ID"))?;
        let mut value = load(tx, playlist_id).await?;
        value.total_count =
            sqlx::query_scalar("SELECT COUNT(*) FROM playlist_entries WHERE playlist_id=?")
                .bind(playlist)
                .fetch_one(&mut **tx)
                .await?;
        value.revision += 1;
        publish(tx, &mut value).await?;
    }
    sqlx::query("UPDATE tracks SET deleted=1,revision=revision+1 WHERE id=?")
        .bind(id.to_string())
        .execute(&mut **tx)
        .await?;
    sync::record(tx, Entity::Track, id, revision + 1, None).await?;
    library::remove_orphans(tx, artist, album).await?;
    Ok(())
}
pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    headers: HeaderMap,
    body: Result<Json<PlaylistCreate>, JsonRejection>,
) -> Result<(StatusCode, Json<Playlist>), AppError> {
    let Json(body) = body.map_err(|_| AppError::invalid("Invalid playlist request"))?;
    let result = mutate(
        &state,
        device.0,
        headers,
        Uuid::new_v4(),
        Action::Create(body),
    )
    .await?
    .ok_or_else(|| AppError::internal("Missing playlist result"))?;
    Ok((StatusCode::CREATED, Json(result)))
}
pub(crate) async fn rename(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<PlaylistRename>, JsonRejection>,
) -> Result<Json<Playlist>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid playlist ID"))?;
    let Json(body) = body.map_err(|_| AppError::invalid("Invalid playlist request"))?;
    Ok(Json(
        mutate(&state, device.0, headers, id, Action::Rename(body))
            .await?
            .ok_or_else(|| AppError::internal("Missing playlist result"))?,
    ))
}
pub(crate) async fn replace(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<PlaylistTracks>, JsonRejection>,
) -> Result<Json<Playlist>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid playlist ID"))?;
    let Json(body) = body.map_err(|_| AppError::invalid("Invalid playlist request"))?;
    Ok(Json(
        mutate(&state, device.0, headers, id, Action::Replace(body))
            .await?
            .ok_or_else(|| AppError::internal("Missing playlist result"))?,
    ))
}
pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<RevisionRequest>, JsonRejection>,
) -> Result<StatusCode, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid playlist ID"))?;
    let Json(body) = body.map_err(|_| AppError::invalid("Invalid playlist request"))?;
    mutate(&state, device.0, headers, id, Action::Delete(body)).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub(crate) async fn delete_track(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    path: Result<Path<Uuid>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<RevisionRequest>, JsonRejection>,
) -> Result<StatusCode, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid track ID"))?;
    let Json(body) = body.map_err(|_| AppError::invalid("Invalid track request"))?;
    mutate(&state, device.0, headers, id, Action::DeleteTrack(body)).await?;
    Ok(StatusCode::NO_CONTENT)
}
