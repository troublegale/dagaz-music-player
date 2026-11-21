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

## A2 playback checkpoint (in progress)

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

Still pending: physical screen lock/headset/audio-focus checks, network loss during
an established stream, next-track prefetch evidence and long/Doze playback.
Playlists, explicit downloads, catalog persistence and device handoff are not
validated or complete in this checkpoint. High/low remain HTTP 425 until S6;
the app offers an explicit original-quality choice rather than falling back.
