use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Original,
    High,
    Low,
}

/// Missing preserves the field; Clear removes its tag; Set replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Patch<T> {
    #[default]
    Missing,
    Clear,
    Set(T),
}
impl<T> Patch<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}
impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Set(value) => value.serialize(serializer),
            _ => serializer.serialize_none(),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(v) => Self::Set(v),
            None => Self::Clear,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataPatch {
    #[serde(default, skip_serializing_if = "Patch::is_missing")]
    pub title: Patch<String>,
    #[serde(default, skip_serializing_if = "Patch::is_missing")]
    pub artist: Patch<String>,
    #[serde(default, skip_serializing_if = "Patch::is_missing")]
    pub album: Patch<String>,
    #[serde(default, skip_serializing_if = "Patch::is_missing")]
    pub album_artist: Patch<String>,
    #[serde(default, skip_serializing_if = "Patch::is_missing")]
    pub track_number: Patch<u16>,
    #[serde(default, skip_serializing_if = "Patch::is_missing")]
    pub disc_number: Patch<u16>,
    #[serde(default, skip_serializing_if = "Patch::is_missing")]
    pub artwork_id: Patch<Uuid>,
}
