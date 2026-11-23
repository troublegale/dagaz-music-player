CREATE TABLE quality_jobs (
    job_id TEXT PRIMARY KEY NOT NULL REFERENCES jobs(id),
    track_id TEXT NOT NULL REFERENCES tracks(id),
    revision INTEGER NOT NULL,
    profile TEXT NOT NULL CHECK(profile IN ('high','low')),
    effective_quality TEXT,
    UNIQUE(track_id,revision,profile)
);
CREATE TABLE download_manifests (
    id TEXT PRIMARY KEY NOT NULL,
    quality TEXT NOT NULL,
    selection_revision INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX manifest_expiry ON download_manifests(expires_at);
CREATE TABLE manifest_items (
    manifest_id TEXT NOT NULL REFERENCES download_manifests(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    track_id TEXT NOT NULL REFERENCES tracks(id),
    revision INTEGER NOT NULL,
    PRIMARY KEY(manifest_id,position)
);
CREATE INDEX manifest_revision_pin ON manifest_items(track_id,revision,manifest_id);
