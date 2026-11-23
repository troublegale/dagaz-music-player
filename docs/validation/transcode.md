# Quality profile probe

Validated locally on Windows/Docker Linux, 2026-11-18. FFmpeg 6.1.1-3ubuntu5
in `dagaz-audio-tools:validation` (the metadata probe's Ubuntu 24.04 image),
Lofty 0.25.5, Rust 1.100.0 MSVC.

## Reproduction

```powershell
cargo test --manifest-path experiments/transcode/Cargo.toml
cargo clippy --manifest-path experiments/transcode/Cargo.toml --all-targets -- -D warnings
cargo run --manifest-path experiments/transcode/Cargo.toml -- INPUT OUTPUT high
```

The CLI accepts `original`, `high`, `low`. Original returns the input path;
it does not create a redundant copy at OUTPUT. A supported MP3 at or below the
target bitrate is also reused. High encodes MP3 at 320 kbit/s, low at 128 kbit/s.
The probe invokes Docker, not a host FFmpeg installation.

## Results

The two tests exercise PCM WAV, float WAV, FLAC and VBR MP3. Converted files retain
artist, album and front cover, and duration differs by at most 500ms. A generated
320k MP3 converts to low quality; equal-bitrate MP3s and low-bitrate originals
are reused. Original bytes stay identical. Existing output files and corrupt
inputs are rejected without publishing a new output or overwriting a file.

Bitrate decisions use FFprobe's audio-stream bitrate, excluding artwork and tag
bytes. Lofty's duration-based average was 349k for a tiny CBR 320k fixture because
of encoder padding; using it would unnecessarily re-encode equal-bitrate inputs.
The regression case checks that both generated profiles are reused when selected
again at the same quality.

FFmpeg reads a read-only bind mount and writes into a fresh temporary directory.
The generated file's tags are mapped to ID3v2 with Lofty and the format/duration
are checked before publication. A same-filesystem hard link creates OUTPUT
without replacing an existing destination. A process-wide mutex serializes probe
FFmpeg calls. This is not the server's persistent job scheduler.

The committed `fixtures/audio/profiles/{high,low}.mp3` are generated from our
30-second synthetic WAV and cover.png. Rodio 0.23.0 decodes both; reported
duration and samples remaining after seeking to 25 seconds stay within 500ms.
Android packages these same fixtures through a Gradle copy task. The physical
Poco X8 Pro/Android 16 run of `QualityProbeTest` passed: both profiles play,
duration stays within 500ms of 30 seconds, and playback advances after seeking
to 25 seconds without a player error. Build and lint also pass.

## Remaining checks

This is not a VPS benchmark: CPU/RAM, cold encode plus RustFS upload time on the
actual Ubuntu VPS, and server job concurrency/restart behavior remain unmeasured.
The probe requires Docker bind-mount-compatible local paths and hard-link support
on the output filesystem. It does not replace S6's authenticated media jobs or
promise a latency bound for a particular VPS tariff. P6 remains partial.

## S6 server checkpoint — 2026-11-23

The production server now queues durable per-track/revision/profile jobs after
import and retag, with a unique database key and one awaited conversion at a time.
It reuses MP3 originals at or below the target bitrate, copies tags/front covers
into MP3 derivatives, validates duration/format and publishes immutable RustFS
objects. Originals remain available throughout. GET/HEAD expose effective quality
and canonical URLs; pending requests expose their stable job ID.

Validation uses native FFmpeg/FFprobe 9.1 on Windows, from the Gyan essentials
build linked by [FFmpeg's download page](https://ffmpeg.org/download.html).
Archive source: [Gyan builds](https://www.gyan.dev/ffmpeg/builds/).
Verified ZIP SHA-256:
`b9bbbb3acece000cbeec994f856649354e52c773807841e78908374b24d658fd`.
Tools stay in ignored `.tools`; they are not added to the product or system PATH.

The new integration tests cover concurrent demand returning the same job,
byte-for-byte MP3 reuse, actual 128 kbit/s stream rate via independent FFprobe,
metadata/front-cover preservation, durable results after restart, timeout/partial
cleanup, recovery after restoring a missing encoder, and skipping deleted unpinned
sources. Other quality jobs and originals survive a permanent conversion failure.

Manifest tests cover a transactional 24-hour snapshot for track/album/playlist,
album order, duplicate playlist occurrences, pagination/cursor scope, membership
changes after creation, metadata revision changes, pending qualities, expiry and
pins retaining deleted/superseded sources. Sources are not silently replaced by a
new revision during a download. Expiry tests use controlled temporary database
timestamps, not a 24-hour wall-clock wait.

On the Poco, production playback passes the nine-test regression and a separate
real Rust/RustFS integration test for original/high/low plus an original alias;
see [Android validation](android-client.md). The new native regression exposed and
fixed a duplicate cache key for requested high/effective original. Explicit offline
downloads are still A3 work.

Remaining release checks: CPU/RAM and encoding time on the actual Ubuntu VPS,
forced OS/service termination, long queues and real metro/Doze playback. Orphan
objects, expired manifest rows and abandoned multipart cleanup remain S8 gates.
One parallel workspace build hit split rlib/rmeta metadata errors in the local
Rust toolchain; a serial `cargo test --workspace -j 1` rebuild passed.

Final workspace verification: 51 passed, zero failed, three opt-in/helper tests
ignored by the standard run. Clippy passes for all workspace targets, including
the Windows UI feature, with warnings denied. Encoder inputs force the stored
audio container and allow only file/pipe protocols; the free-space check budgets
for expanded output before encoding compressed sources.
