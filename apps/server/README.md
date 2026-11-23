# Personal library server

Current scope: health, device pairing/revocation, resumable uploads, durable
imports and authenticated original-audio streaming. Catalog browsing is next.

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

## Immutable media

`GET` and `HEAD /api/v1/tracks/{id}/media?revision=1&quality=original` require
authentication, an explicit positive revision and quality. Single byte ranges,
suffix ranges and open-ended ranges are supported. Matching `If-Range` returns
206; mismatch returns the full entity with 200. Invalid ranges return 416 with
`Content-Range: bytes */LENGTH`. `If-Match` mismatch returns 412.

ETag and `X-Content-SHA256` identify the entire stored file; response length and
Content-Range describe the returned selection. HEAD checks S3 metadata without
downloading the object. Content-Disposition encodes untrusted filenames safely.
The response streams with backpressure and a 64 KiB reader buffer. At most 16
outbound media bodies are held at once; permits and revision leases are released
on completion, cancellation or upstream failure. This is a buffer bound, not a
measurement of total process RAM.

An old revision remains readable for 24 hours after `superseded_at`; expired or
removed revisions return 410. In-flight bodies retain a lease for the future S8
garbage collector. Object cleanup itself is not implemented yet. High/low use
durable representation jobs and return 425 with a job ID while pending, as
described below. The shared descriptor helper also serves download manifests.

Tests cover a real import-to-media download with byte equality, revision changes,
Range/If-Range/If-Match, HEAD, filename safety, and resource release. Held response
bodies are tested directly through the real router and S3 to make cancellation
and capacity checks deterministic rather than dependent on TCP buffer sizes.

## Catalog and synchronization

Authenticated catalog routes list tracks, artists, albums and playlists in pages
of 1–500 items (default 100). Track search treats input as literal words with
prefix matching, including Cyrillic; SQL/FTS expressions are not accepted as code.
Track sorts are `title`, `artist`, `album`; album contents default to disc/track
order. Named lists use `sort=name`. `artist_id` filters tracks by performer or
album credit, and albums by album credit. Cursors belong to their query/filter.

Albums are identified by normalized name and album artist (falling back to the
performer). Tracks retain their original tag text and stable IDs. Import links
credited names to artist IDs; this does not disambiguate real-world people.
Each metadata text field is limited to 4 KiB. Existing imports are indexed in
bounded batches on startup after the catalog migration.

Playlist create/rename/replace/delete and track deletion require a UUID
`Idempotency-Key`; edits/deletes also require `expected_revision`. Conflicts
return 409. Repeated track IDs preserve separate ordered occurrences. Replacing
contents accepts a JSON body up to 16 MiB. GET playlist pages are fenced by its
revision: an old cursor returns 409 after an edit. Lists and sync payloads contain
playlist summaries with a cursor to the first member page; they do not embed an
unbounded track list. Deleting a playlist leaves tracks intact; deleting a track
removes all its playlist occurrences, advances those playlist revisions and emits
tombstones. Unreferenced album/artist catalog entries are also tombstoned. Physical
audio cleanup remains S8 work.

Start synchronization with `/api/v1/snapshot`. Keep its `generation` and
`high_watermark` while following `next_cursor`, then request
`/api/v1/changes?cursor=GENERATION:HIGH_WATERMARK`. Follow each delta's
`next_cursor`. Apply an entity only when its `sequence` is newer than the locally
stored sequence, retaining deletion sequences too. Snapshot pages may contain
concurrent updates newer than their starting watermark; this rule prevents older
deltas from overwriting them. Changes are committed with the catalog transaction.
Invalidated generations or sequences below the retained floor return 410
`cursor_expired`, requiring a new snapshot. The journal currently retains all
changes; pruning with at least 30 days of retention and restore-generation rotation
will be implemented in S8. WebSocket notifications/presence are grouped with S7;
clients can poll HTTP changes in the meantime.

Run the isolated RustFS fixture first (see `experiments/storage/run.ps1`), then:

```text
cargo test -p dagaz-server
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p dagaz-server --test library catalog_pages_one_hundred_thousand_tracks -- --ignored --nocapture
```

Tests use a real loopback HTTP listener, separate SQLite files and a unique empty
S3 bucket per test. They read test credentials from ignored `deploy/.env.test`
or `DAGAZ_TEST_S3_*` environment variables and only target loopback S3. Successful
tests remove their own buckets. They do not touch a user library.

Public HTTPS, installer, backups and service packaging are release work. Device
presence currently reports offline; WebSocket sessions and immediate closure on
revocation will be tested when S7 introduces those sessions. Linux SIGTERM
handling is implemented, but this stage's executed checks were on Windows.

## Metadata and artwork (S5)

`PATCH /api/v1/tracks/{id}/metadata` accepts `expected_revision` and `patch`;
send a UUID `Idempotency-Key`. Missing patch fields preserve their current values,
`null` clears them, and a nonempty value replaces them. Text fields accept up to
4096 UTF-8 bytes each. Reusing the same key and request returns the same Job;
a changed request with that key or a stale single-track revision returns 409.

`POST /api/v1/tracks/metadata-batch` accepts 1–100 distinct
`{ track_id, expected_revision }` items and a common patch, also with an idempotency
key. Both endpoints return 202 and a Job. Poll `/api/v1/jobs/{id}`. Its
`result.items` contains each track's `status` and either `revision`/`track` or an
`error`. Mixed success yields `partial`; a failed file does not roll back other
successful files. Revision/CAS, catalog membership, search, changes and durable
per-file progress commit together. A retry never publishes the same edit twice.

Upload covers as binary `image/jpeg` or `image/png` to `POST /api/v1/artworks`;
201 returns `{ id }`. Limits are 10 MiB and 25 million pixels; decoding is checked
before creating a thumbnail. Reading requires authorization:
`GET /api/v1/artworks/{id}` returns the original, and `?size=256` returns a PNG
fitting within 256×256. URLs are not fetched. Only one HTTP cover upload is
processed at a time; excess concurrent uploads return 429 for retry. The import
worker can process one additional embedded cover independently.

WAV RIFF/ID3, MP3 ID3v2 and FLAC Vorbis edits operate on a local working copy,
then publish a new immutable S3 key. Existing playback keeps the old revision;
superseded versions retain the S3 read path's 24-hour grace period. Only the front
cover is replaced or cleared; other picture types are preserved. Import indexes
embedded JPEG/PNG front covers. Album artwork is derived from the first covered
track in album order and is updated in the same catalog transaction.

Validation: `cargo test -p dagaz-server --test tags` uses real RustFS and the
existing `dagaz-audio-tools:validation` Docker image for independent PCM checks.
Orphan candidates/unused artworks and abandoned multipart uploads await S8's
grace-period collector; this checkpoint does not enable production deployment.

## Quality variants and download snapshots (S6)

Install FFmpeg with libmp3lame and FFprobe for the server. Defaults are `ffmpeg`
and `ffprobe` on PATH; `DAGAZ_FFMPEG` / `DAGAZ_FFPROBE` can select executable paths.
`DAGAZ_MEDIA_TIMEOUT_SECONDS` defaults to 300 and accepts 1–3600 seconds. Originals
remain available when tools are missing; the affected jobs retry with backoff.

Import and retag enqueue high (MP3 320 kbit/s) and low (MP3 128 kbit/s) once per
track/revision/profile. The worker awaits each probe/encode in sequence; import
and metadata jobs take priority over queued transcodes. Tool arguments use no
shell, input formats are fixed to WAV/MP3/FLAC and network protocols are disabled.
On timeout the child is killed and awaited before continuing; partial files are
removed. Permanent conversion failures end that job, leaving originals and other
jobs usable. S3 failures and unavailable tools retry the same durable job.

An MP3 whose FFprobe audio bitrate is already at or below the chosen target reuses
the original byte-for-byte. `requested_quality` and `effective_quality` distinguish
this reuse. Ready media replies include `X-Effective-Quality` and a canonical
`Content-Location`; derived bytes have `audio/mpeg` and an MP3 filename. Pending
GET/HEAD returns 425 with `X-Job-Id` (and `ApiError.job_id` in a GET body). A failed
quality returns 422 with that job ID. Readiness updates the catalog change sequence
without changing the file revision. WebSocket hints remain S7 work; clients can
poll jobs and catalog changes.

`POST /api/v1/download-manifests` takes `{ kind, id, quality }` and a UUID
`Idempotency-Key`, returning 201 `{ id }`. `kind` is `track`, `album` or `playlist`.
Membership and exact file revisions are copied in one SQLite transaction and
expire after 24 hours. Playlist order and duplicate occurrences are retained;
album order follows the catalog. Playlist selection revision is its captured
revision; track selection revision is its file revision; albums use the captured
catalog high watermark because album DTOs have no membership revision.

Read `GET /api/v1/download-manifests/{id}?limit=100&cursor=...` (maximum 500).
Descriptors contain final length, SHA-256, ETag, effective quality and canonical
URL. A pending representation returns 425/job ID; retry the same page after it is
ready. Playlist or tag edits cannot change existing manifest members/revisions.
Unexpired manifests pin a deleted or superseded source. Expired manifests return
410; explicitly create a new one with a new operation key. Reusing an old key
returns the same manifest, including its original expiry.

Tests: `cargo test -p dagaz-server --test qualities --test manifests`.
Windows test defaults use ignored `.tools/ffmpeg/ffmpeg-9.1-essentials_build/bin`;
`DAGAZ_TEST_FFMPEG` / `DAGAZ_TEST_FFPROBE` override those paths. On Linux tests use
PATH. Client download storage and shared offline pins remain A3/W3 work.
