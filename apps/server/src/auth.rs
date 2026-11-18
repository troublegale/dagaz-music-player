use crate::{AppError, AppState};
use axum::{
    Extension, Json,
    extract::{
        ConnectInfo, Path, Query, Request, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use dagaz_protocol::{Device, Page, PairRequest, PairResponse, Platform};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

fn now() -> Result<i64, AppError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppError::internal("System clock must be after Unix epoch"))?
        .as_secs() as i64)
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn random_hex(bytes: usize) -> Result<String, AppError> {
    let mut value = vec![0; bytes];
    getrandom::fill(&mut value)
        .map_err(|_| AppError::internal("Secure random generator unavailable"))?;
    Ok(value.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub async fn new_pairing_code(db: &SqlitePool) -> Result<String, AppError> {
    let code = random_hex(8)?;
    let timestamp = now()?;
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM pairing_codes WHERE expires_at < ?")
        .bind(timestamp - 86400)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO pairing_codes(code_hash,created_at,expires_at) VALUES(?,?,?)")
        .bind(hash(&code))
        .bind(timestamp)
        .bind(timestamp + 600)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(code)
}

#[derive(Hash, Eq, PartialEq)]
enum LimitKey {
    Pair(IpAddr),
    Device(Uuid),
}
#[derive(Default)]
pub(crate) struct RateLimits(Mutex<HashMap<LimitKey, (Instant, u32)>>);
impl RateLimits {
    fn check(&self, key: LimitKey, maximum: u32, timestamp: Instant) -> Result<(), AppError> {
        let mut entries = self
            .0
            .lock()
            .map_err(|_| AppError::internal("Request limiter unavailable"))?;
        entries.retain(|_, (start, _)| timestamp.duration_since(*start) < Duration::from_secs(60));
        if entries.len() >= 4096 && !entries.contains_key(&key) {
            return Err(AppError::rate_limited());
        }
        let (_, count) = entries.entry(key).or_insert((timestamp, 0));
        if *count >= maximum {
            return Err(AppError::rate_limited());
        }
        *count += 1;
        Ok(())
    }
}

pub(crate) async fn pair(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Result<Json<PairRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<PairResponse>), AppError> {
    state
        .limits
        .check(LimitKey::Pair(peer.ip()), 10, Instant::now())?;
    let Json(request) = body.map_err(|_| AppError::invalid("Invalid pairing request"))?;
    let name = request.device_name.trim();
    if name.is_empty()
        || name.chars().count() > 128
        || name.chars().any(char::is_control)
        || request.pairing_code.len() > 128
    {
        return Err(AppError::invalid("Invalid device name or pairing code"));
    }
    let timestamp = now()?;
    let id = Uuid::new_v4();
    let token = random_hex(32)?;
    let platform = match request.platform {
        Platform::Windows => "windows",
        Platform::Android => "android",
    };
    let mut tx = state.db.begin().await?;
    let consumed = sqlx::query("UPDATE pairing_codes SET used_at=? WHERE code_hash=? AND used_at IS NULL AND expires_at>? RETURNING code_hash")
        .bind(timestamp).bind(hash(request.pairing_code.trim())).bind(timestamp).fetch_optional(&mut *tx).await?;
    if consumed.is_none() {
        return Err(AppError::unauthorized());
    }
    sqlx::query(
        "INSERT INTO devices(id,token_hash,device_name,platform,created_at) VALUES(?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(hash(&token))
    .bind(name)
    .bind(platform)
    .bind(timestamp)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(PairResponse {
            device_id: id,
            token,
        }),
    ))
}

#[derive(Clone, Copy)]
pub(crate) struct Identity(pub Uuid);
pub(crate) async fn authorize(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .ok_or_else(AppError::unauthorized)?;
    let (scheme, token) = header.split_once(' ').ok_or_else(AppError::unauthorized)?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.len() != 64
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::unauthorized());
    }
    let id: Option<String> =
        sqlx::query_scalar("SELECT id FROM devices WHERE token_hash=? AND revoked_at IS NULL")
            .bind(hash(token))
            .fetch_optional(&state.db)
            .await?;
    let id = id
        .ok_or_else(AppError::unauthorized)?
        .parse()
        .map_err(|_| AppError::internal("Invalid device identity in database"))?;
    state
        .limits
        .check(LimitKey::Device(id), 300, Instant::now())?;
    request.extensions_mut().insert(Identity(id));
    Ok(next.run(request).await)
}

#[derive(serde::Deserialize)]
pub(crate) struct DeviceQuery {
    cursor: Option<Uuid>,
    limit: Option<u32>,
}
pub(crate) async fn devices(
    State(state): State<Arc<AppState>>,
    Extension(identity): Extension<Identity>,
    query: Result<Query<DeviceQuery>, QueryRejection>,
) -> Result<Json<Page<Device>>, AppError> {
    let _requesting_device = identity.0;
    let Query(query) = query.map_err(|_| AppError::invalid("Invalid device page query"))?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(AppError::invalid("Page limit must be between 1 and 500"));
    }
    let rows: Vec<(String,String,String)> = sqlx::query_as("SELECT id,device_name,platform FROM devices WHERE revoked_at IS NULL AND id>? ORDER BY id LIMIT ?")
        .bind(query.cursor.map(|id| id.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&state.db).await?;
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    for (id, device_name, platform) in rows.into_iter().take(limit as usize) {
        let platform = match platform.as_str() {
            "windows" => Platform::Windows,
            "android" => Platform::Android,
            _ => return Err(AppError::internal("Invalid device platform in database")),
        };
        items.push(Device {
            id: id
                .parse()
                .map_err(|_| AppError::internal("Invalid device identity in database"))?,
            device_name,
            platform,
            online: false,
        });
    }
    let next_cursor = if more {
        items.last().map(|device| device.id.to_string())
    } else {
        None
    };
    Ok(Json(Page { items, next_cursor }))
}

pub(crate) async fn revoke(
    State(state): State<Arc<AppState>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, AppError> {
    let Path(id) = path.map_err(|_| AppError::invalid("Invalid device ID"))?;
    sqlx::query("UPDATE devices SET revoked_at=COALESCE(revoked_at,?) WHERE id=?")
        .bind(now()?)
        .bind(id.to_string())
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limiter_expires_windows_and_separates_keys() {
        let limits = RateLimits::default();
        let timestamp = Instant::now();
        for _ in 0..10 {
            limits
                .check(LimitKey::Pair("127.0.0.1".parse().unwrap()), 10, timestamp)
                .unwrap();
        }
        assert!(
            limits
                .check(LimitKey::Pair("127.0.0.1".parse().unwrap()), 10, timestamp)
                .is_err()
        );
        limits
            .check(LimitKey::Pair("127.0.0.2".parse().unwrap()), 10, timestamp)
            .unwrap();
        limits
            .check(
                LimitKey::Pair("127.0.0.1".parse().unwrap()),
                10,
                timestamp + Duration::from_secs(60),
            )
            .unwrap();
    }
}
