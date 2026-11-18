use crate::{AppError, AppState};
use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::StatusCode,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use dagaz_protocol::{Album, Artist, Entity, Page, Track};
use sqlx::{QueryBuilder, Sqlite, SqlitePool, Transaction};
use std::sync::Arc;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

fn missing() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", "Catalog item not found")
}
fn id(value: &str) -> Result<Uuid, AppError> {
    value
        .parse()
        .map_err(|_| AppError::internal("Invalid stored catalog ID"))
}
pub(crate) fn normal(value: &str) -> String {
    value
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn text(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
pub(crate) fn encode<T: serde::Serialize>(value: &T) -> Result<String, AppError> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(value)?))
}
pub(crate) fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, AppError> {
    if value.len() > 65536 {
        return Err(AppError::invalid("Cursor is too long"));
    }
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| AppError::invalid("Invalid cursor"))?,
    )
    .map_err(|_| AppError::invalid("Invalid cursor"))
}
pub(crate) fn validate_track(track: &Track) -> Result<(), AppError> {
    if [
        &track.title,
        &track.artist,
        &track.album,
        &track.album_artist,
    ]
    .into_iter()
    .flatten()
    .any(|value| {
        value.len() > 4096
            || value
                .chars()
                .any(|character| character.is_control() && !character.is_whitespace())
    }) {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_metadata",
            "Metadata text exceeds 4 KiB or contains invalid controls",
        ));
    }
    Ok(())
}
async fn artist(tx: &mut Transaction<'_, Sqlite>, name: &str) -> Result<Uuid, AppError> {
    let existing: Option<(String, String, i64, i64)> =
        sqlx::query_as("SELECT id,name,revision,deleted FROM artists WHERE normalized_name=?")
            .bind(normal(name))
            .fetch_optional(&mut **tx)
            .await?;
    let (artist_id, display, revision, changed) =
        if let Some((artist_id, display, revision, deleted)) = existing {
            (id(&artist_id)?, display, revision + deleted, deleted != 0)
        } else {
            (Uuid::new_v4(), name.trim().to_owned(), 1, true)
        };
    if changed {
        let value = Artist {
            id: artist_id,
            name: display,
        };
        sqlx::query("INSERT INTO artists(id,normalized_name,name,data_json,revision) VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET deleted=0,revision=excluded.revision,data_json=excluded.data_json").bind(artist_id.to_string()).bind(normal(name)).bind(&value.name).bind(serde_json::to_string(&value)?).bind(revision).execute(&mut **tx).await?;
        crate::sync::record(
            tx,
            Entity::Artist,
            artist_id,
            revision,
            Some(serde_json::to_value(value)?),
        )
        .await?;
    }
    Ok(artist_id)
}
async fn album(
    tx: &mut Transaction<'_, Sqlite>,
    name: &str,
    credit: Option<&str>,
    artist_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    let key = serde_json::to_string(&(normal(name), credit.map(normal)))?;
    let existing: Option<(String, String, i64, i64)> =
        sqlx::query_as("SELECT id,data_json,revision,deleted FROM albums WHERE identity_key=?")
            .bind(&key)
            .fetch_optional(&mut **tx)
            .await?;
    let (album_id, value, revision, changed) =
        if let Some((album_id, payload, revision, deleted)) = existing {
            (
                id(&album_id)?,
                serde_json::from_str::<Album>(&payload)?,
                revision + deleted,
                deleted != 0,
            )
        } else {
            let album_id = Uuid::new_v4();
            (
                album_id,
                Album {
                    id: album_id,
                    name: name.trim().to_owned(),
                    album_artist: credit.map(str::to_owned),
                    album_artist_id: artist_id,
                    artwork_id: None,
                },
                1,
                true,
            )
        };
    if changed {
        sqlx::query("INSERT INTO albums(id,identity_key,name,normalized_name,album_artist_id,data_json,revision) VALUES(?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET deleted=0,revision=excluded.revision,data_json=excluded.data_json").bind(album_id.to_string()).bind(key).bind(&value.name).bind(format!("{}\0{}",normal(name),credit.map(normal).unwrap_or_default())).bind(artist_id.map(|id|id.to_string())).bind(serde_json::to_string(&value)?).bind(revision).execute(&mut **tx).await?;
        crate::sync::record(
            tx,
            Entity::Album,
            album_id,
            revision,
            Some(serde_json::to_value(value)?),
        )
        .await?;
    }
    Ok(album_id)
}
pub(crate) async fn write_track(
    tx: &mut Transaction<'_, Sqlite>,
    track: &mut Track,
) -> Result<(), AppError> {
    validate_track(track)?;
    track.artist_id = match text(&track.artist) {
        Some(name) => Some(artist(tx, name).await?),
        None => None,
    };
    track.album_id = if let Some(name) = text(&track.album) {
        let credit = text(&track.album_artist).or_else(|| text(&track.artist));
        let credit_id = if let Some(credit) = credit {
            Some(artist(tx, credit).await?)
        } else {
            None
        };
        Some(album(tx, name, credit, credit_id).await?)
    } else {
        None
    };
    let title = normal(text(&track.title).unwrap_or(&track.filename));
    let artist = normal(text(&track.artist).unwrap_or(""));
    let album = normal(text(&track.album).unwrap_or(""));
    let credit = normal(
        text(&track.album_artist)
            .or_else(|| text(&track.artist))
            .unwrap_or(""),
    );
    let order = format!(
        "{:05}:{:05}\0{title}",
        track.disc_number.map(u32::from).unwrap_or(65536),
        track.track_number.map(u32::from).unwrap_or(65536)
    );
    sqlx::query("INSERT INTO tracks(id,revision,filename,data_json,artist_id,album_id,sort_title,sort_artist,sort_album,sort_album_order,catalog_ready) VALUES(?,?,?,?,?,?,?,?,?,?,1) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,filename=excluded.filename,data_json=excluded.data_json,artist_id=excluded.artist_id,album_id=excluded.album_id,sort_title=excluded.sort_title,sort_artist=excluded.sort_artist,sort_album=excluded.sort_album,sort_album_order=excluded.sort_album_order,catalog_ready=1")
        .bind(track.id.to_string()).bind(track.revision).bind(&track.filename).bind(serde_json::to_string(track)?).bind(track.artist_id.map(|id|id.to_string())).bind(track.album_id.map(|id|id.to_string())).bind(&title).bind(format!("{artist}\0{album}\0{credit}\0{order}")).bind(format!("{album}\0{credit}\0{order}")).bind(order).execute(&mut **tx).await?;
    crate::sync::record(
        tx,
        Entity::Track,
        track.id,
        track.revision,
        Some(serde_json::to_value(track)?),
    )
    .await?;
    Ok(())
}
pub(crate) async fn backfill(db: &SqlitePool) -> Result<(), AppError> {
    loop {
        let rows:Vec<String>=sqlx::query_scalar("SELECT data_json FROM tracks WHERE catalog_ready=0 AND deleted=0 ORDER BY id LIMIT 200").fetch_all(db).await?;
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
        for row in rows {
            write_track(&mut tx, &mut serde_json::from_str::<Track>(&row)?).await?;
        }
        tx.commit().await?;
    }
}
pub(crate) async fn remove_orphans(
    tx: &mut Transaction<'_, Sqlite>,
    artist: Option<String>,
    album: Option<String>,
) -> Result<(), AppError> {
    let mut artists = Vec::new();
    if let Some(artist) = artist {
        artists.push(artist);
    }
    if let Some(album) = album {
        let row:Option<(i64,Option<String>)>=sqlx::query_as("UPDATE albums SET deleted=1,revision=revision+1 WHERE id=? AND deleted=0 AND NOT EXISTS(SELECT 1 FROM tracks WHERE album_id=? AND deleted=0) RETURNING revision,album_artist_id").bind(&album).bind(&album).fetch_optional(&mut **tx).await?;
        if let Some((revision, credit)) = row {
            if let Some(credit) = credit {
                artists.push(credit);
            }
            crate::sync::record(tx, Entity::Album, id(&album)?, revision, None).await?;
        }
    }
    for artist in artists {
        let revision:Option<i64>=sqlx::query_scalar("UPDATE artists SET deleted=1,revision=revision+1 WHERE id=? AND deleted=0 AND NOT EXISTS(SELECT 1 FROM tracks WHERE artist_id=? AND deleted=0) AND NOT EXISTS(SELECT 1 FROM albums WHERE album_artist_id=? AND deleted=0) RETURNING revision").bind(&artist).bind(&artist).bind(&artist).fetch_optional(&mut **tx).await?;
        if let Some(revision) = revision {
            crate::sync::record(tx, Entity::Artist, id(&artist)?, revision, None).await?;
        }
    }
    Ok(())
}

#[derive(serde::Deserialize, Default)]
pub(crate) struct ListQuery {
    q: Option<String>,
    sort: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
    artist_id: Option<Uuid>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Cursor {
    scope: String,
    q: String,
    sort: String,
    key: String,
    id: Uuid,
}
impl ListQuery {
    fn limit(&self) -> Result<usize, AppError> {
        let limit = self.limit.unwrap_or(100);
        if !(1..=500).contains(&limit) {
            return Err(AppError::invalid("Limit must be between 1 and 500"));
        }
        Ok(limit as usize)
    }
    fn search(&self) -> Result<String, AppError> {
        let value = self.q.as_deref().unwrap_or("");
        if value.len() > 1024
            || value
                .chars()
                .any(|character| character.is_control() && !character.is_whitespace())
        {
            return Err(AppError::invalid("Invalid search query"));
        }
        Ok(normal(value))
    }
}
fn literal_match(value: &str) -> Option<String> {
    let terms: Vec<_> = value
        .split_whitespace()
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}
async fn track_page(
    state: &AppState,
    query: ListQuery,
    album: Option<Uuid>,
) -> Result<Page<Track>, AppError> {
    let limit = query.limit()?;
    let search = query.search()?;
    let sort = query.sort.as_deref().unwrap_or(if album.is_some() {
        "album_order"
    } else {
        "title"
    });
    let column = match sort {
        "title" => "t.sort_title",
        "artist" => "t.sort_artist",
        "album" => "t.sort_album",
        "album_order" if album.is_some() => "t.sort_album_order",
        _ => return Err(AppError::invalid("Unknown track sort")),
    };
    let scope = format!("tracks:{album:?}:{:?}", query.artist_id);
    let cursor = if let Some(value) = &query.cursor {
        let cursor: Cursor = decode(value)?;
        if cursor.scope != scope || cursor.q != search || cursor.sort != sort {
            return Err(AppError::invalid("Cursor does not match the query"));
        }
        Some(cursor)
    } else {
        None
    };
    let mut sql = QueryBuilder::<Sqlite>::new(format!(
        "SELECT t.data_json,{column} FROM tracks t WHERE t.deleted=0"
    ));
    if let Some(album) = album {
        sql.push(" AND t.album_id=").push_bind(album.to_string());
    }
    if let Some(artist) = query.artist_id {
        sql.push(" AND (t.artist_id=")
            .push_bind(artist.to_string())
            .push(" OR t.album_id IN (SELECT id FROM albums WHERE deleted=0 AND album_artist_id=")
            .push_bind(artist.to_string())
            .push("))");
    }
    if !search.is_empty() {
        let Some(expression) = literal_match(&search) else {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            });
        };
        sql.push(" AND t.rowid IN (SELECT rowid FROM track_search WHERE track_search MATCH ")
            .push_bind(expression)
            .push(")");
    }
    if let Some(cursor) = cursor {
        sql.push(format!(" AND ({column},t.id)>("))
            .push_bind(cursor.key)
            .push(",")
            .push_bind(cursor.id.to_string())
            .push(")");
    }
    sql.push(format!(" ORDER BY {column},t.id LIMIT "))
        .push_bind((limit + 1) as i64);
    let rows: Vec<(String, String)> = sql.build_query_as().fetch_all(&state.db).await?;
    let more = rows.len() > limit;
    let mut items = Vec::new();
    let mut next = None;
    for (payload, key) in rows.into_iter().take(limit) {
        let track: Track = serde_json::from_str(&payload)?;
        if more {
            next = Some(encode(&Cursor {
                scope: scope.clone(),
                q: search.clone(),
                sort: sort.into(),
                key,
                id: track.id,
            })?);
        }
        items.push(track);
    }
    Ok(Page {
        items,
        next_cursor: next,
    })
}
pub(crate) async fn tracks(
    State(state): State<Arc<AppState>>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Page<Track>>, AppError> {
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid catalog query"))?;
    Ok(Json(track_page(&state, query, None).await?))
}
pub(crate) async fn track(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Track>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid track ID"))?;
    let payload: Option<String> =
        sqlx::query_scalar("SELECT data_json FROM tracks WHERE id=? AND deleted=0")
            .bind(id.to_string())
            .fetch_optional(&state.db)
            .await?;
    Ok(Json(serde_json::from_str(&payload.ok_or_else(missing)?)?))
}
pub(crate) async fn album_tracks(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Page<Track>>, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid album ID"))?;
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid catalog query"))?;
    if !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM albums WHERE id=? AND deleted=0)",
    )
    .bind(id.to_string())
    .fetch_one(&state.db)
    .await?
    {
        return Err(missing());
    }
    Ok(Json(track_page(&state, query, Some(id)).await?))
}
pub(crate) async fn named_page<T: serde::de::DeserializeOwned>(
    state: &AppState,
    query: ListQuery,
    table: &str,
) -> Result<Page<T>, AppError> {
    let limit = query.limit()?;
    let search = query.search()?;
    if query.sort.as_deref().is_some_and(|sort| sort != "name") {
        return Err(AppError::invalid("Unknown catalog sort"));
    }
    let scope = format!("{table}:{:?}", query.artist_id);
    let cursor = if let Some(value) = query.cursor {
        let cursor: Cursor = decode(&value)?;
        if cursor.scope != scope || cursor.q != search || cursor.sort != "name" {
            return Err(AppError::invalid("Cursor does not match the query"));
        }
        Some(cursor)
    } else {
        None
    };
    let mut sql = QueryBuilder::<Sqlite>::new(format!(
        "SELECT id,data_json,normalized_name FROM {table} WHERE deleted=0"
    ));
    if !search.is_empty() {
        sql.push(" AND instr(normalized_name,")
            .push_bind(&search)
            .push(")>0");
    }
    if let Some(cursor) = cursor {
        sql.push(" AND (normalized_name,id)>(")
            .push_bind(cursor.key)
            .push(",")
            .push_bind(cursor.id.to_string())
            .push(")");
    }
    if table == "albums"
        && let Some(artist) = query.artist_id
    {
        sql.push(" AND album_artist_id=")
            .push_bind(artist.to_string());
    }
    sql.push(" ORDER BY normalized_name,id LIMIT ")
        .push_bind((limit + 1) as i64);
    let rows: Vec<(String, String, String)> = sql.build_query_as().fetch_all(&state.db).await?;
    let more = rows.len() > limit;
    let mut items = Vec::new();
    let mut next = None;
    for (stored_id, payload, key) in rows.into_iter().take(limit) {
        if more {
            next = Some(encode(&Cursor {
                scope: scope.clone(),
                q: search.clone(),
                sort: "name".into(),
                key,
                id: id(&stored_id)?,
            })?);
        }
        items.push(serde_json::from_str(&payload)?);
    }
    Ok(Page {
        items,
        next_cursor: next,
    })
}
pub(crate) async fn artists(
    State(state): State<Arc<AppState>>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Page<Artist>>, AppError> {
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid catalog query"))?;
    Ok(Json(named_page(&state, query, "artists").await?))
}
pub(crate) async fn albums(
    State(state): State<Arc<AppState>>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Page<Album>>, AppError> {
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid catalog query"))?;
    Ok(Json(named_page(&state, query, "albums").await?))
}
