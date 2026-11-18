CREATE TABLE tracks (
    id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    filename TEXT NOT NULL,
    data_json TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE media_revisions (
    track_id TEXT NOT NULL REFERENCES tracks(id),
    revision INTEGER NOT NULL,
    quality TEXT NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    byte_length INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    etag TEXT NOT NULL,
    storage_etag TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(track_id,revision,quality)
);
CREATE INDEX media_checksum ON media_revisions(sha256);
CREATE TABLE uploads (
    id TEXT PRIMARY KEY NOT NULL,
    filename TEXT NOT NULL,
    total_bytes INTEGER NOT NULL CHECK(total_bytes > 0),
    sha256 TEXT NOT NULL,
    offset INTEGER NOT NULL DEFAULT 0 CHECK(offset >= 0),
    status TEXT NOT NULL DEFAULT 'uploading',
    expires_at INTEGER NOT NULL,
    job_id TEXT
);
CREATE TABLE jobs (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    upload_id TEXT REFERENCES uploads(id),
    candidate_track_id TEXT,
    result_json TEXT,
    errors_json TEXT NOT NULL DEFAULT '[]',
    attempts INTEGER NOT NULL DEFAULT 0,
    not_before INTEGER NOT NULL DEFAULT 0,
    s3_upload_id TEXT,
    progress_bytes INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX pending_jobs ON jobs(status,not_before);
CREATE TABLE idempotency (
    device_id TEXT NOT NULL REFERENCES devices(id),
    route TEXT NOT NULL,
    key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(device_id,route,key)
);
CREATE TABLE changes (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    entity TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    payload_json TEXT,
    created_at INTEGER NOT NULL
);
