# Personal library server

Current scope: health, device pairing/revocation, resumable uploads and durable
imports. Media streaming and catalog browsing are later plan tasks.

Required environment:

| Variable | Value |
| --- | --- |
| `DAGAZ_S3_ENDPOINT` | Private RustFS HTTP(S) origin, e.g. `http://127.0.0.1:19000` for tests |
| `DAGAZ_S3_BUCKET` | Existing private bucket; checked at server startup |
| `DAGAZ_S3_ACCESS_KEY` | S3 access key |
| `DAGAZ_S3_SECRET_KEY` | S3 secret key |
| `DAGAZ_DATA_DIR` | Persistent local SQLite directory; default `data` |
| `DAGAZ_LISTEN` | IP and port; default `127.0.0.1:8080` |
| `DAGAZ_MAX_UPLOAD_BYTES` | Optional per-file bound; default 8 GiB, no track-count limit |
| `DAGAZ_MIN_FREE_BYTES` | Optional staging safety reserve; default 128 MiB |

From the repository root:

```text
cargo run -p dagaz-server
cargo run -p dagaz-server -- pair-code
```

The installed binary is named `dagaz`; `dagaz pair-code` prints a new pairing
code. Treat that output as a secret. A code expires after ten minutes and can be
consumed once, including concurrent attempts. Codes and device tokens are stored
only as SHA-256 hashes. Tokens contain 256 bits from the OS random generator.

`POST /api/v1/pair` accepts the shared `PairRequest` and returns `PairResponse`.
Use `Authorization: Bearer TOKEN` for `GET /api/v1/devices` and
`DELETE /api/v1/devices/{id}`. Listing accepts `limit` (1–500, default 100) and
`cursor` from the previous page. Revocation is idempotent; a revoked token remains
invalid after restart. Any paired device belongs to this one personal owner and
can manage the owner's other devices. There are no separate users or roles yet.

Pairing is limited to ten attempts per minute per TCP peer IP; protected requests
to 300 per minute per device. Rejections use 429 and `Retry-After: 60`. The limiter
uses monotonic one-minute windows and bounded in-memory state; restart resets the
windows, not credentials or revocations. Forwarded-IP headers are deliberately
not trusted, so requests through the same reverse proxy share its pairing quota.

SQLite uses WAL, foreign keys, full synchronous commits and a busy timeout. Both
SQLite initialization and the configured S3 bucket check must succeed before
HTTP starts. Application errors have the shared `ApiError` shape and exclude
raw backend errors, credentials, pairing codes and tokens. `/healthz` is public.

## Upload and import

`POST /api/v1/uploads` accepts `UploadCreate` and a UUID `Idempotency-Key` and
returns 201. `GET /api/v1/uploads/{id}` returns the committed offset. `PATCH`
accepts up to 8 MiB of `application/octet-stream` with `Upload-Offset`,
`X-Chunk-SHA256` and a UUID `Idempotency-Key`. An exact retry is safe; changing the
operation behind a used key returns 409. A repeated create returns the same
upload's current state. File names are labels; filesystem and S3 keys use UUIDs.

`POST /api/v1/uploads/{id}/complete` returns 202 and a durable `Job`.
`GET /api/v1/jobs/{id}` reports completion or errors; success includes `track_id`.
Repeating completion returns the same job. Unfinished uploads expire after 24
hours; incomplete files never appear in the library. Chunks are synced before
their offset is committed. After a crash, uncommitted tail bytes are truncated.

The single worker verifies the entire file's SHA-256 with bounded reads and
inspects its content with Lofty. Only WAV, MP3 and FLAC are accepted. It uploads
the unchanged original through multipart S3 with 8 MiB parts, records the upload
ID and resumes existing parts after restart. It also recognizes a complete
object left by a crash before database publication. Track/revision, change event,
job result and upload completion are then committed together. Known checksums of
active tracks/revisions return the existing track, regardless of filename.

Transient S3 failures keep the job queued with bounded backoff. File corruption
fails the job without publishing a track. A failed validation requires a new
upload. The worker completes its current operation during orderly shutdown;
queued jobs persist. Run one application process per library database.

SQLite serializes chunk writes through short `BEGIN IMMEDIATE` transactions.
This keeps per-upload retry/offset handling atomic without an in-memory lock
registry; concurrent writes to different uploads also serialize. Disk checks
and 507 errors protect staging, but real disk-fault injection and orphan/expired
file cleanup remain S8 work. The configurable reserve is a free-space guard,
not a claim that all outstanding uploads fit at once.

Run the isolated RustFS fixture first (see `experiments/storage/run.ps1`), then:

```text
cargo test -p dagaz-server
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Tests use a real loopback HTTP listener, separate SQLite files and a unique empty
S3 bucket per test. They read test credentials from ignored `deploy/.env.test`
or `DAGAZ_TEST_S3_*` environment variables and only target loopback S3. Successful
tests remove their own buckets. They do not touch a user library.

Public HTTPS, installer, backups and service packaging are release work. Device
presence currently reports offline; WebSocket sessions and immediate closure on
revocation will be tested when S7 introduces those sessions. Linux SIGTERM
handling is implemented, but this stage's executed checks were on Windows.
