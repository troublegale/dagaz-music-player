use crate::{
    AppError, AppState,
    auth::Identity,
    operations::{key, recall, remember},
};
use axum::{
    Extension, Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
};
use dagaz_protocol::{ManifestCreate, ManifestCreated, ManifestPage, Quality, SelectionKind};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;
pub(crate) async fn pinned(state: &AppState, id: Uuid, revision: i64) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM manifest_items i JOIN download_manifests m ON m.id=i.manifest_id WHERE i.track_id=? AND i.revision=? AND m.expires_at>?)").bind(id.to_string()).bind(revision).bind(crate::unix_time()?).fetch_one(&state.db).await?)
}
pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    Extension(device): Extension<Identity>,
    headers: HeaderMap,
    body: Result<Json<ManifestCreate>, JsonRejection>,
) -> Result<(StatusCode, Json<ManifestCreated>), AppError> {
    let Json(request) = body.map_err(|_| AppError::invalid("Invalid manifest request"))?;
    let key = key(&headers)?;
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&request)?));
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(id) = recall(&mut tx, device.0, "manifest", &key, &fingerprint).await? {
        tx.commit().await?;
        return Ok((
            StatusCode::CREATED,
            Json(ManifestCreated {
                id: id
                    .parse()
                    .map_err(|_| AppError::internal("Invalid manifest ID"))?,
            }),
        ));
    }
    let selection: Option<i64> = match request.kind {
        SelectionKind::Track => {
            sqlx::query_scalar("SELECT revision FROM tracks WHERE id=? AND deleted=0")
                .bind(request.id.to_string())
                .fetch_optional(&mut *tx)
                .await?
        }
        SelectionKind::Playlist => {
            sqlx::query_scalar("SELECT revision FROM playlists WHERE id=? AND deleted=0")
                .bind(request.id.to_string())
                .fetch_optional(&mut *tx)
                .await?
        }
        SelectionKind::Album => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM albums WHERE id=? AND deleted=0)")
                    .bind(request.id.to_string())
                    .fetch_one(&mut *tx)
                    .await?;
            if exists {
                Some(
                    sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(sequence),1) FROM changes")
                        .fetch_one(&mut *tx)
                        .await?,
                )
            } else {
                None
            }
        }
    };
    let selection = selection
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Selection not found"))?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO download_manifests VALUES(?,?,?,?)")
        .bind(id.to_string())
        .bind(crate::qualities::name(request.quality))
        .bind(selection)
        .bind(crate::unix_time()? + 86400)
        .execute(&mut *tx)
        .await?;
    let sql = match request.kind {
        SelectionKind::Track => {
            "INSERT INTO manifest_items SELECT ?,0,id,revision FROM tracks WHERE id=? AND deleted=0"
        }
        SelectionKind::Album => {
            "INSERT INTO manifest_items SELECT ?,ROW_NUMBER() OVER(ORDER BY sort_album_order,id)-1,id,revision FROM tracks WHERE album_id=? AND deleted=0"
        }
        SelectionKind::Playlist => {
            "INSERT INTO manifest_items SELECT ?,e.position,t.id,t.revision FROM playlist_entries e JOIN tracks t ON t.id=e.track_id WHERE e.playlist_id=? AND t.deleted=0"
        }
    };
    sqlx::query(sql)
        .bind(id.to_string())
        .bind(request.id.to_string())
        .execute(&mut *tx)
        .await?;
    remember(
        &mut tx,
        device.0,
        "manifest",
        &key,
        &fingerprint,
        &id.to_string(),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(ManifestCreated { id })))
}
#[derive(serde::Deserialize, Default)]
pub(crate) struct PageQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Cursor {
    manifest: Uuid,
    position: i64,
}
pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<ManifestPage>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid manifest ID"))?;
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid manifest page"))?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(AppError::invalid("Limit must be between 1 and 500"));
    }
    let after = if let Some(cursor) = query.cursor {
        let cursor: Cursor = crate::library::decode(&cursor)?;
        if cursor.manifest != id || cursor.position < 0 {
            return Err(AppError::invalid("Invalid manifest cursor"));
        }
        cursor.position
    } else {
        -1
    };
    let row: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT quality,selection_revision,expires_at FROM download_manifests WHERE id=?",
    )
    .bind(id.to_string())
    .fetch_optional(&state.db)
    .await?;
    let (quality, selection_revision, expires) =
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "Manifest not found"))?;
    if expires <= crate::unix_time()? {
        return Err(AppError::new(
            StatusCode::GONE,
            "manifest_expired",
            "Manifest expired; create a fresh snapshot",
        ));
    }
    let quality: Quality = serde_json::from_value(serde_json::Value::String(quality))?;
    let rows:Vec<(i64,String,i64)>=sqlx::query_as("SELECT position,track_id,revision FROM manifest_items WHERE manifest_id=? AND position>? ORDER BY position LIMIT ?").bind(id.to_string()).bind(after).bind(i64::from(limit)+1).fetch_all(&state.db).await?;
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    let mut last = None;
    for (position, track, revision) in rows.into_iter().take(limit as usize) {
        let track = track
            .parse()
            .map_err(|_| AppError::internal("Invalid manifest track ID"))?;
        items.push(crate::media::descriptor(&state, track, revision, quality).await?);
        last = Some(position);
    }
    let next_cursor = if more {
        last.map(|position| {
            crate::library::encode(&Cursor {
                manifest: id,
                position,
            })
        })
        .transpose()?
    } else {
        None
    };
    Ok(Json(ManifestPage {
        items,
        next_cursor,
        selection_revision,
    }))
}
