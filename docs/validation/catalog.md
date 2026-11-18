# Catalog checkpoint

Executed on the development Windows host against loopback HTTP, SQLite WAL and
an isolated local RustFS bucket. These are debug builds, not VPS measurements.

- Real tagged FLAC imports: Cyrillic search and literal FTS punctuation, stable
  artist IDs, same-name albums with different album credits, disc/track ordering.
- Playlist duplicate occurrences, paging, stale-revision/cursor conflicts,
  idempotent retries, rejected missing tracks without partial changes, deletion
  of all occurrences and tombstones for unused artists/albums.
- Snapshot paged across concurrent creation, update, deletion and server restart,
  then delta replay by sequence: resulting reference catalog equals the server.
  Expired journal floors and changed generations return 410.
- `cargo test --workspace`: 29 tests pass; the large measurement is opt-in.
- `cargo clippy --workspace --all-targets -- -D warnings` and format check pass.

Explicit measurement:

```text
cargo test -p dagaz-server --test library catalog_pages_one_hundred_thousand_tracks -- --ignored --nocapture
```

100,000 synthetic metadata rows (not 100,000 audio imports): insertion/indexing
2.159 s; 200 track-list HTTP pages 3.034 s; exact FTS search 21.75 ms; 200 snapshot
HTTP pages 10.094 s. Each response holds at most 500 records. The test checks
monotonic IDs and exact counts without accumulating the catalog in client RAM.
Total process RAM was not profiled. R2 remains responsible for release/VPS
performance and larger hostile-input measurements.

WebSocket notifications are grouped with S7. Journal pruning, restore-generation
rotation and physical object cleanup are S8 work; currently all changes remain.
