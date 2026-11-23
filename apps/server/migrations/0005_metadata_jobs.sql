CREATE TABLE artworks (
    id TEXT PRIMARY KEY NOT NULL,
    mime TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_length INTEGER NOT NULL,
    thumbnail_sha256 TEXT NOT NULL,
    thumbnail_length INTEGER NOT NULL
);
CREATE TABLE metadata_jobs (
    job_id TEXT PRIMARY KEY NOT NULL REFERENCES jobs(id),
    patch_json TEXT NOT NULL
);
CREATE TABLE metadata_items (
    job_id TEXT NOT NULL REFERENCES metadata_jobs(job_id),
    position INTEGER NOT NULL,
    track_id TEXT NOT NULL,
    expected_revision INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    result_json TEXT,
    PRIMARY KEY(job_id,position)
);
