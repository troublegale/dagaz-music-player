use crate::AppError;
use axum::http::{HeaderMap, StatusCode};
use sqlx::{Sqlite, Transaction};
use uuid::Uuid;
pub(crate) fn key(headers: &HeaderMap) -> Result<String, AppError> {
    Ok(headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::invalid("Idempotency-Key is required"))?
        .parse::<Uuid>()
        .map_err(|_| AppError::invalid("Idempotency-Key must be a UUID"))?
        .to_string())
}
pub(crate) async fn recall(
    tx: &mut Transaction<'_, Sqlite>,
    device: Uuid,
    route: &str,
    key: &str,
    fingerprint: &str,
) -> Result<Option<String>, AppError> {
    let stored: Option<(String, String)> = sqlx::query_as(
        "SELECT fingerprint,result_id FROM idempotency WHERE device_id=? AND route=? AND key=?",
    )
    .bind(device.to_string())
    .bind(route)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?;
    match stored {
        Some((saved, id)) if saved == fingerprint => Ok(Some(id)),
        Some(_) => Err(AppError::new(
            StatusCode::CONFLICT,
            "conflict",
            "Idempotency key was used for a different request",
        )),
        None => Ok(None),
    }
}
pub(crate) async fn remember(
    tx: &mut Transaction<'_, Sqlite>,
    device: Uuid,
    route: &str,
    key: &str,
    fingerprint: &str,
    id: &str,
) -> Result<(), AppError> {
    sqlx::query("INSERT INTO idempotency(device_id,route,key,fingerprint,result_id,created_at) VALUES(?,?,?,?,?,?)").bind(device.to_string()).bind(route).bind(key).bind(fingerprint).bind(id).bind(crate::unix_time()?).execute(&mut **tx).await?;
    Ok(())
}
