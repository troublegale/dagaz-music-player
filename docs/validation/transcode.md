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
