use crate::AppError;
use crate::{AppState, library};
use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
    http::StatusCode,
};
use dagaz_protocol::Entity;
use dagaz_protocol::{Change, SnapshotPage, SyncPage};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) async fn initialize(db: &SqlitePool) -> Result<(), AppError> {
    sqlx::query(
        "INSERT OR IGNORE INTO server_meta(key,value) VALUES('generation',?),('sync_floor','0')",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(db)
    .await?;
    Ok(())
}
pub(crate) async fn record(
    tx: &mut Transaction<'_, Sqlite>,
    entity: Entity,
    id: Uuid,
    revision: i64,
    payload: Option<serde_json::Value>,
) -> Result<i64, AppError> {
    let (name, table) = match entity {
        Entity::Track => ("track", "tracks"),
        Entity::Artist => ("artist", "artists"),
        Entity::Album => ("album", "albums"),
        Entity::Playlist => ("playlist", "playlists"),
        _ => return Err(AppError::internal("Unsupported catalog change entity")),
    };
    let deleted = payload.is_none();
    let payload = payload
        .map(|value| serde_json::to_string(&value))
        .transpose()?;
    let sequence:i64=sqlx::query_scalar("INSERT INTO changes(entity,entity_id,revision,deleted,payload_json,created_at) VALUES(?,?,?,?,?,?) RETURNING sequence").bind(name).bind(id.to_string()).bind(revision).bind(deleted).bind(payload).bind(crate::unix_time()?).fetch_one(&mut **tx).await?;
    sqlx::query(&format!("UPDATE {table} SET last_sequence=? WHERE id=?"))
        .bind(sequence)
        .bind(id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(sequence)
}
#[derive(serde::Deserialize)]
pub(crate) struct SyncQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}
impl SyncQuery {
    fn limit(&self) -> Result<i64, AppError> {
        let limit = self.limit.unwrap_or(100);
        if !(1..=500).contains(&limit) {
            return Err(AppError::invalid("Limit must be between 1 and 500"));
        }
        Ok(i64::from(limit))
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotCursor {
    generation: Uuid,
    high_watermark: i64,
    kind: String,
    id: Uuid,
}
fn expired() -> AppError {
    AppError::new(
        StatusCode::GONE,
        "cursor_expired",
        "Sync cursor expired; fetch a new snapshot",
    )
}
async fn metadata(tx: &mut Transaction<'_, Sqlite>) -> Result<(Uuid, i64, i64), AppError> {
    let generation: String =
        sqlx::query_scalar("SELECT value FROM server_meta WHERE key='generation'")
            .fetch_one(&mut **tx)
            .await?;
    let floor: String = sqlx::query_scalar("SELECT value FROM server_meta WHERE key='sync_floor'")
        .fetch_one(&mut **tx)
        .await?;
    let floor: i64 = floor
        .parse()
        .map_err(|_| AppError::internal("Invalid sync floor"))?;
    let high: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0) FROM changes")
        .fetch_one(&mut **tx)
        .await?;
    Ok((
        generation
            .parse()
            .map_err(|_| AppError::internal("Invalid sync generation"))?,
        floor,
        high.max(floor),
    ))
}
type ChangeRow = (i64, String, String, i64, bool, Option<String>);
fn change(row: ChangeRow) -> Result<Change, AppError> {
    let (sequence, entity, entity_id, revision, deleted, payload) = row;
    Ok(Change {
        sequence,
        entity: serde_json::from_value(serde_json::Value::String(entity))?,
        entity_id: entity_id
            .parse()
            .map_err(|_| AppError::internal("Invalid change entity ID"))?,
        revision,
        deleted,
        payload: payload
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
    })
}
pub(crate) async fn snapshot(
    State(state): State<Arc<AppState>>,
    query: Result<Query<SyncQuery>, QueryRejection>,
) -> Result<Json<SnapshotPage>, AppError> {
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid snapshot query"))?;
    let limit = query.limit()?;
    let mut tx = state.db.begin().await?;
    let (generation, floor, current) = metadata(&mut tx).await?;
    let cursor: Option<SnapshotCursor> =
        query.cursor.as_deref().map(library::decode).transpose()?;
    if let Some(cursor) = &cursor {
        if cursor.generation != generation
            || cursor.high_watermark < floor
            || cursor.high_watermark > current
        {
            return Err(expired());
        }
        if !["album", "artist", "playlist", "track"].contains(&cursor.kind.as_str()) {
            return Err(AppError::invalid("Invalid snapshot cursor"));
        }
    }
    let high_watermark = cursor
        .as_ref()
        .map_or(current, |cursor| cursor.high_watermark);
    // Rows can be newer than the watermark; applying changes by sequence reconciles concurrent edits.
    let mut sql = sqlx::QueryBuilder::<Sqlite>::new(
        "SELECT last_sequence,kind,id,revision,deleted,data_json FROM (SELECT last_sequence,'album' AS kind,id,revision,deleted,data_json FROM albums UNION ALL SELECT last_sequence,'artist',id,revision,deleted,data_json FROM artists UNION ALL SELECT last_sequence,'playlist',id,revision,deleted,data_json FROM playlists UNION ALL SELECT last_sequence,'track',id,revision,deleted,data_json FROM tracks WHERE catalog_ready=1) WHERE deleted=0",
    );
    if let Some(cursor) = cursor {
        sql.push(" AND (kind,id)>(")
            .push_bind(cursor.kind)
            .push(",")
            .push_bind(cursor.id.to_string())
            .push(")");
    }
    sql.push(" ORDER BY kind,id LIMIT ").push_bind(limit + 1);
    let rows: Vec<ChangeRow> = sql.build_query_as().fetch_all(&mut *tx).await?;
    let more = rows.len() > limit as usize;
    let mut records = Vec::new();
    let mut next_cursor = None;
    for row in rows.into_iter().take(limit as usize) {
        if more {
            next_cursor = Some(library::encode(&SnapshotCursor {
                generation,
                high_watermark,
                kind: row.1.clone(),
                id: row
                    .2
                    .parse()
                    .map_err(|_| AppError::internal("Invalid snapshot entity ID"))?,
            })?);
        }
        records.push(change(row)?);
    }
    tx.commit().await?;
    Ok(Json(SnapshotPage {
        generation,
        high_watermark,
        records,
        next_cursor,
    }))
}
pub(crate) async fn changes(
    State(state): State<Arc<AppState>>,
    query: Result<Query<SyncQuery>, QueryRejection>,
) -> Result<Json<SyncPage>, AppError> {
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid changes query"))?;
    let limit = query.limit()?;
    let cursor = query
        .cursor
        .ok_or_else(|| AppError::invalid("Changes cursor is required"))?;
    if cursor.len() > 128 {
        return Err(AppError::invalid("Invalid changes cursor"));
    }
    let (generation, after) = cursor
        .split_once(':')
        .ok_or_else(|| AppError::invalid("Invalid changes cursor"))?;
    let generation: Uuid = generation
        .parse()
        .map_err(|_| AppError::invalid("Invalid changes generation"))?;
    let after: i64 = after
        .parse()
        .map_err(|_| AppError::invalid("Invalid changes sequence"))?;
    if after < 0 {
        return Err(AppError::invalid("Invalid changes sequence"));
    }
    let mut tx = state.db.begin().await?;
    let (current, floor, high) = metadata(&mut tx).await?;
    if current != generation || after < floor || after > high {
        return Err(expired());
    }
    let rows:Vec<ChangeRow>=sqlx::query_as("SELECT sequence,entity,entity_id,revision,deleted,payload_json FROM changes WHERE sequence>? ORDER BY sequence LIMIT ?").bind(after).bind(limit+1).fetch_all(&mut *tx).await?;
    let has_more = rows.len() > limit as usize;
    let changes: Vec<Change> = rows
        .into_iter()
        .take(limit as usize)
        .map(change)
        .collect::<Result<_, _>>()?;
    let next_cursor = format!(
        "{generation}:{}",
        changes.last().map_or(after, |change| change.sequence)
    );
    tx.commit().await?;
    Ok(Json(SyncPage {
        generation,
        changes,
        next_cursor,
        has_more,
    }))
}
