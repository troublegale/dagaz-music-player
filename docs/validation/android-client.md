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
