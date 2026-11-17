//! Shared wire contract for the personal music library.
mod library;
mod playback;
pub use library::*;
pub use playback::*;
mod models;
pub use models::*;

pub(crate) fn nonnegative<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let n = <i64 as serde::Deserialize>::deserialize(d)?;
    if n < 0 {
        Err(serde::de::Error::custom("must be nonnegative"))
    } else {
        Ok(n)
    }
}
pub(crate) fn positive<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let n = nonnegative(d)?;
    if n == 0 {
        Err(serde::de::Error::custom("must be positive"))
    } else {
        Ok(n)
    }
}
mod schema;
pub use schema::openapi_document;
