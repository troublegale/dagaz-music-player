# Windows audio probe

Work in progress, Windows x64, Rust 1.100.0 MSVC, 2026-11-18.
Versions: GPUI Kit 0.10.0, Rodio 0.23.0 (WAV/MP3/FLAC, playback only),
stream-download 0.24.6, Reqwest 0.13.5. Exact resolution is in the probe lockfile.

## Reproduction

From the repository root, start the shared synthetic HTTP fixture helper:

```powershell
node experiments/media-server/server.mjs
```

Then in another terminal:

```powershell
cargo test --manifest-path experiments/windows-audio/Cargo.toml
cargo run --manifest-path experiments/windows-audio/Cargo.toml --features ui
```

The optional `ui` feature keeps the large GPUI dependency out of transport-only
test builds. The window is a technical probe, not the product's visual design.
It offers Cyrillic search over three fixtures, a native file picker, network WAV
with a forced disconnect, pause/resume and seeking to ten seconds. It only reads
selected local files; it does not upload or modify them.

## Transport decision

Use the library's background downloader with temporary-file storage for this
probe. Reads and seeks are synchronous for Rodio, while HTTP runs on Tokio.
This avoids loading an entire large WAV into RAM and avoids writing a bespoke
download/cache implementation. The file is temporary, not an offline download.

The library's HTTP adapter does not validate resumed entity/range headers itself.
The probe supplies a small strict client: require finite positive Content-Length,
byte ranges and a strong ETag; pin If-Match; reject changed ETag, ignored ranges,
incorrect Content-Range and changed response length. A three-second connect
timeout and five-second read timeout bound individual HTTP operations.

Four tests passed: throttled 30-second WAV starts decoding before the whole file
arrives, all expected samples survive the injected disconnect, decoder seek to
25 seconds leaves the expected five seconds within a 500ms tolerance, and VBR
MP3/FLAC decode over HTTP. Unknown length, missing range support, changed revision,
ignored ranges and incorrect range offsets are rejected in the test cases.

`cargo check --features ui`, the debug build, `cargo fmt --check`, and Clippy
with `--all-targets --features ui -- -D warnings` passed.

The native window was opened and exercised through Windows UI automation:
Cyrillic text `Тестовый исполнитель` appeared in the input and filtered out the
fixture rows; network playback reached the playing state, the pause button
changed it to paused, and the system file dialog selected `tone.flac`, which
returned the player to its playing state without an error. Audio device setup
succeeded. Audible output still needs human confirmation; playing state alone
does not prove sound reached the listener.

## Limits

There is no product disk budget or cache eviction yet. A temporary file can grow
to the declared source length. Descriptor acquisition uses HEAD for this probe;
the product will consume the server's authenticated revision descriptor.

Audio loading/decoder preparation occurs on a worker thread. The GUI stays
independent, but controls can queue behind preparation; cancellation and bounded
control latency remain W2 work. This prototype is not a reusable product engine.

Audible output, system media buttons, unsupported device formats, Windows 10
compatibility and optimized release performance remain unverified. P2 is partial.
