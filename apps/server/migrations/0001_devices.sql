CREATE TABLE devices (
    id TEXT PRIMARY KEY NOT NULL,
    token_hash TEXT UNIQUE NOT NULL,
    device_name TEXT NOT NULL CHECK(length(device_name) BETWEEN 1 AND 128),
    platform TEXT NOT NULL CHECK(platform IN ('windows', 'android')),
    created_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE TABLE pairing_codes (
    code_hash TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER
);
CREATE INDEX pairing_codes_expiration ON pairing_codes(expires_at);
