# Android connection checkpoint

2026-11-21, Poco X8 Pro, Android 16/API 36. This is the production client package
`io.dagaz.player`, separate from the earlier `io.dagaz.probe.media` audio probe.

Executed checks:

- Four JVM tests pass: all shared JSON fixture messages round-trip, duplicate
  playback occurrences/order survive, absent/null metadata edits stay distinct,
  invalid patch/playback values are rejected, pairing/auth work, unsafe origins,
  redirects and oversized responses are rejected.
- `PairingTest.credentialRestore` passes on real Android Keystore: encrypted
  save/replacement/restore, non-exportable key, no raw token in stored JSON or
  session string output, invalid code/revocation, ciphertext tampering and missing
  key. A new pairing can save again after key loss.
- `ConnectionScreenTest.pairingSurvivesRecreationAndHandlesRevocation` passes on
  the phone: Compose input/pairing, Activity recreation, close/new Activity,
  restored connection and re-pair screen after revocation with address preserved.
  The two device tests completed together in 4.643 s on the final UI build.
- `ServerPairingTest.pairsWithRealServerAndHandlesRevocation` passes in 0.202 s:
  real Rust HTTP + SQLite + isolated RustFS, USB loopback forwarding, one-time code,
  Keystore restoration, repeated-code refusal and revoked-token refusal. Its Rust
  companion fixture also passes and removes its own readiness/secret file and bucket.
- Debug and unsigned release APK builds and both lint tasks pass. Lint still
  reports dependency-update/version-catalog suggestions; pinned versions are kept.
- Rust workspace: 32 tests pass; the live Android fixture is opt-in. Clippy with
  Windows UI enabled and format checks pass.

Debugging evidence:

- Initial JVM run failed on missing models/API; initial Android test compilation
  failed on the missing Activity. Shared contract tests then passed.
- JUnit rejected one inferred non-Unit test return; the test was corrected.
- Network-security lint required explicit `includeSubdomains=false`.
- Streaming ADB installation was refused without a prompt. The user enabled the
  relevant USB installation permissions; push installation (`--no-streaming`)
  succeeded for both the main and test packages. No security setting was changed
  programmatically.
- Device MockWebServer used a hostname where the debug client accepts loopback
  IPs. Binding/addressing `127.0.0.1` made the fixture deterministic.
- The keyboard covered the test's click target. The Compose tree showed zero HTTP
  requests and an offscreen button; scrolling it into view made the UI test pass.

The connection form is visually checked on the phone, uses system insets and a
scrolling body, and keeps the agreed dark/green styling without the removed
Android top search or "Твоя музыка" heading. Codes are memory-only; the encrypted
session is stored atomically outside backup, and release cleartext is disabled.

Not checked here: public VPS/HTTPS deployment, Android API 28, other phone vendors,
system font/locale variants, power loss during an atomic write or signing and
distribution. The library and product playback service are A2 work. Earlier media
probe tests do not establish production-client playback, offline or handoff readiness.

## A2 playback checkpoint

2026-11-21, same Poco/API 36, production MediaSessionService with ExoPlayer.
Three device tests pass together in 146.772 s: generated 30-second WAV and shared
synthetic MP3/FLAC fixtures. They verify authenticated revision-pinned streaming,
early WAV playback before the full response is buffered, seek to a duplicate queue
occurrence, one player with multiple controllers, repeat/shuffle, Activity
recreation and continued playback with the Activity stopped. Short compressed
fixtures repeat for the background check; this is not a long-duration soak test.
Debug/JVM checks and unsigned release assembly/lint pass (0 errors, 26 existing
dependency/version-catalog suggestions). JVM coverage now includes paged Unicode
catalog requests, HEAD descriptors, pending quality and bounded retry delays.

Debugging: a test helper incorrectly nested runOnMainSync; removing that nested
call fixed the test process crash. The mock media server then returned 200 for a
bounded Range starting at byte zero, correctly rejected by the production header
validator. The fixture now returns exact 206/Content-Range/length for all ranges;
the three-format device run passes. No production validation was relaxed.

The final playback run passes 4/4 in 196.975 s, adding recovery from an initial
HTTP 503 while preserving a user pause until explicit play. Connection/Keystore
and UI regression pass 2/2 in 6.022 s, including a Cyrillic search request and
visible result through the dedicated Search tab. The Library tab has no search
field. No tests in these runs were skipped. The device runner now rejects skipped
or assumption-failed tests instead of treating their JUnit summary as success.

The final extended device suite passes 8/8 in 38.697 s, without skipped tests:
the prior checks plus transient audio-focus loss/gain with user-pause preservation,
interruption halfway through an HTTP response and recovery using a nonzero Range,
a distinct next-track request before the current track ends, and paused prepare at
1000 ms. The latter exposed a real bug: separate setMediaItem/seek commands lost
the requested position during session item resolution. Passing the initial
position with setMediaItem fixes it; the same test now passes.

Physical checks also pass on the production service: Bluetooth disconnect 1/1
in 10.702 s (device disappears, playback pauses, user hears no speaker fallback),
screen lock 1/1 in 19.754 s (15 seconds with screen off, position advances, one
player, user hears no stutter). No power key or headset event was injected.
An earlier extended run failed MP3 Activity recreation after the phone entered
Doze. Automatic tests now hold a bounded screen wake lock, excluding the explicit
lock test; they never wake or unlock the phone. Returning an already interactive
Activity to the foreground before cleanup avoids this Poco's delayed destruction
of stopped Activities. Final debug/JVM/lint and unsigned release/lint pass.

Still pending for release: long/Doze playback, extended real mobile-network
outages and other devices/API levels. This short screen-off check is not a Doze
soak test.
Playlists, explicit downloads, catalog persistence and device handoff are not
validated or complete in this checkpoint. At A2 high/low returned HTTP 425 until
S6; the app offered an explicit original choice rather than falling back.

## S6 integration — 2026-11-23

The device regression passes 9/9. The added test queues a
different track with requested high/effective original and verifies a canonical
original cache key with no duplicate high key. Before the fix this test failed
and the JVM test read high as the effective quality. Resolution now runs before
CacheDataSource selects its key, and descriptor pruning keeps canonical aliases.
The queued descriptor first returns HTTP 425, then becomes ready; this exposed
and fixed a terminal-error classification that previously stopped the queue.
Pending preparation now uses the existing bounded-backoff retry policy.
Origin/path/revision/quality checks still run before cache or authenticated reads.
Seven JVM tests pass, including header validation and a clear message for a
terminal quality-generation failure. Debug and unsigned release builds/lint pass.

`ServerPlaybackTest` passes 1/1 against real Rust HTTP + SQLite +
isolated RustFS through USB forwarding. The fixture imports a synthetic 30-second
WAV, generates both MP3 profiles using native FFmpeg, and imports an existing low
bitrate MP3. The production service plays original/high/low, advances after a
1000 ms start, and plays the reused original for requested high with one cache
representation. No personal connection or music is used. The fixture and test
both clean their credentials/session and temporary server resources.

The final pipeline also passes both pairing/screen tests and unsigned release
build/lint. The credential guard covers the main connection file and AtomicFile
`.bak`/`.new` files; a regression check uses synthetic files and verifies that
the guard preserves their contents. No personal session is removed to run tests.

These are short functional checks, not a metro/Doze soak or a public VPS test.
Explicit offline downloads, persistent catalog and device handoff remain later work.

## A3 offline checkpoint — 2026-11-28

The Android client now persists catalog, playlist membership, sync generation/
cursor, manifest snapshots, pins and local media in Room. Snapshot and membership
pages stage before publication. DownloadService uses one shared DownloadManager
and a separate cache with no eviction; each personal server origin has its own
Room catalog. Rebinding the same origin keeps the local library. Offline reads
prefer the verified local representation, including in other playlists/albums.
Changing streaming quality does not replace that representation.

The five device offline tests cover shared pins, removing one selection, an
interrupted response followed by a nonzero Range, hash verification and failed
replacement, a reopened database and reads making no HTTP requests, Cyrillic
search, tombstones, staged snapshot promotion, a pending second manifest page,
stable manifest creation, duplicate playlist entries and the service's index
across server origins. Repeated changes to one playlist in a sync page now
coalesce before fetching membership; an advanced membership revision triggers a
fresh snapshot instead of trapping the cursor on an old version. Malformed
payload identities are rejected with the data/cursor transaction rolled back.
A same-URL/new-hash replacement regression failed before
SHA-256 was added to explicit-cache keys: failed verification removed ready
bytes. It now preserves the old copy. Ready publication uses a transaction and
checks the candidate again so a concurrent tombstone cannot publish it.
Late span commits from an already open streaming writer are discarded after
explicit publication; removing the last pin restores normal stream caching.
Both late duplication and mismatched sync payload identity failed regression
checks before their fixes. Final offline regression: 5/5 in 2.402 s.

The shared-index regression also failed before moving the cache/manager to one
runtime. DownloadService retains one manager per service class; creating one per
server left the foreground service attached to the previous server's index.
Cleanup now touches only the current origin's requests. Network retry delays
reuse the streaming schedule (1–30 seconds with jitter); permanent HTTP/version/
cache-write errors stop. The JVM disk-error check injects CacheException/ENOSPC;
the phone's actual storage was not filled.

Eleven JVM tests pass, including the common quality-policy fixture, same-origin
relative manifest URL normalization, foreign-origin rejection, retaining the
library scope after reauthorization, outage retries and terminal disk errors.
The Windows contract reader passes the same policy examples; Windows offline
implementation is still W3 work. Workspace verification passes 52 tests, with
three standard opt-in/helper exclusions. Clippy passes with warnings denied.
Debug and unsigned release builds and lint pass.

The pairing/screen regression passes 2/2 and playback passes 9/9 (44.798 s).
Revoked network access now retains the library/downloads entry point; protected
server operations still return authentication errors. Existing personal sessions
and AtomicFile backups remain protected by the instrumentation guard.

`ServerOfflineTest` uses the real Rust/SQLite/RustFS service. It downloads an
album and a playlist containing repeated/shared tracks through WorkManager and
Media3, verifies two ready representations and saved cover thumbnails, then ends
instrumentation. The driver removes only this fixture's USB reverse tunnel and
starts another instrumentation process. The test asserts a different process ID,
confirms the server is unreachable, opens the stored catalog/covers, plays the
local queue using the production MediaSessionService and removes an album without
losing playlist files. Fixture credentials, marker and server resources are
cleaned up. Both phases pass (2.144 s download, 2.123 s cold playback), including
embedded cover import and thumbnail persistence. This is a disconnected-server cold start; it does not toggle the
phone's airplane mode or claim a real metro test.

Remaining release gates: long mobile-network/Doze runs, OEM background restart
and dataSync timeout behavior, other devices/API levels, large-library UI memory/
latency and unreferenced local cover cleanup. Visited pages/download summaries
are currently materialized in memory; Paging is required before the R3 large-
library validation. This checkpoint does not complete device handoff or editing.
