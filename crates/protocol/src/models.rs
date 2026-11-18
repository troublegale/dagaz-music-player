use crate::{MetadataPatch, Quality, Repeat};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Android,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PairRequest {
    pub pairing_code: String,
    pub device_name: String,
    pub platform: Platform,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PairResponse {
    pub device_id: Uuid,
    pub token: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub request_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Track {
    pub id: Uuid,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub revision: i64,
    pub filename: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    #[serde(default)]
    pub artist_id: Option<Uuid>,
    pub album_id: Option<Uuid>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub duration_ms: i64,
    pub format: AudioFormat,
    pub artwork_id: Option<Uuid>,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub original_bytes: i64,
    pub ready_qualities: Vec<Quality>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Playlist {
    pub id: Uuid,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub revision: i64,
    pub name: String,
    pub track_ids: Vec<Uuid>,
    pub next_cursor: Option<String>,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub total_count: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MediaDescriptor {
    pub track_id: Uuid,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub revision: i64,
    pub requested_quality: Quality,
    pub effective_quality: Quality,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub byte_length: i64,
    pub sha256: String,
    pub etag: String,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    Uploading,
    Processing,
    Completed,
    Failed,
    Expired,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UploadState {
    pub id: Uuid,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub offset: i64,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub total_bytes: i64,
    pub expires_at: String,
    pub status: UploadStatus,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Import,
    Retag,
    Transcode,
    Cleanup,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Partial,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Job {
    pub id: Uuid,
    pub kind: JobKind,
    pub status: JobStatus,
    pub result: Option<Value>,
    pub errors: Vec<ApiError>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Entity {
    Track,
    Album,
    Artist,
    Playlist,
    Artwork,
    Device,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Change {
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub sequence: i64,
    pub entity: Entity,
    pub entity_id: Uuid,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub revision: i64,
    pub deleted: bool,
    pub payload: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SyncPage {
    pub generation: Uuid,
    pub changes: Vec<Change>,
    pub next_cursor: String,
    pub has_more: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MetadataEdit {
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub expected_revision: i64,
    pub patch: MetadataPatch,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlaybackCommand {
    pub operation_id: Uuid,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub expected_epoch: i64,
    #[serde(flatten)]
    pub action: PlaybackAction,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueueUpdate {
    pub queue: Vec<Uuid>,
    pub play_order: Vec<usize>,
    pub queue_index: Option<usize>,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub position_ms: i64,
    pub repeat: Repeat,
    pub shuffle_seed: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", content = "payload", rename_all = "snake_case")]
pub enum PlaybackAction {
    PlayHere(QueueUpdate),
    Pause,
    Resume,
    Seek {
        #[serde(deserialize_with = "crate::nonnegative")]
        #[schemars(range(min = 0))]
        position_ms: i64,
    },
    Next,
    Previous,
    ReplaceQueue(QueueUpdate),
    SetModes {
        repeat: Repeat,
        shuffle_seed: Option<i64>,
        play_order: Vec<usize>,
    },
    Transfer {
        target_device_id: Uuid,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlaybackReport {
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub epoch: i64,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub sequence: i64,
    pub track_id: Uuid,
    pub queue_index: usize,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub position_ms: i64,
    pub paused: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AckPhase {
    Ready,
    Stopped,
    Started,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlaybackAck {
    pub operation_id: Uuid,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub epoch: i64,
    pub phase: AckPhase,
    pub position_ms: Option<i64>,
    pub error: Option<ApiError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Album {
    pub id: Uuid,
    pub name: String,
    pub album_artist: Option<String>,
    #[serde(default)]
    pub album_artist_id: Option<Uuid>,
    pub artwork_id: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Artist {
    pub id: Uuid,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Device {
    pub id: Uuid,
    pub device_name: String,
    pub platform: Platform,
    pub online: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UploadCreate {
    pub filename: String,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub total_bytes: i64,
    #[schemars(regex(pattern = "^[a-fA-F0-9]{64}$"))]
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RevisionRequest {
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlaylistCreate {
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlaylistRename {
    pub name: String,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlaylistTracks {
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub expected_revision: i64,
    pub track_ids: Vec<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TrackRevision {
    pub track_id: Uuid,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MetadataBatch {
    #[schemars(length(min = 1, max = 100))]
    pub items: Vec<TrackRevision>,
    pub patch: MetadataPatch,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectionKind {
    Track,
    Album,
    Playlist,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ManifestCreate {
    pub kind: SelectionKind,
    pub id: Uuid,
    pub quality: Quality,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ManifestCreated {
    pub id: Uuid,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ManifestPage {
    pub items: Vec<MediaDescriptor>,
    pub next_cursor: Option<String>,
    #[serde(deserialize_with = "crate::positive")]
    #[schemars(range(min = 1))]
    pub selection_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArtworkCreated {
    pub id: Uuid,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotPage {
    pub generation: Uuid,
    #[serde(deserialize_with = "crate::nonnegative")]
    #[schemars(range(min = 0))]
    pub high_watermark: i64,
    pub records: Vec<Change>,
    pub next_cursor: Option<String>,
}
