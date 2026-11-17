# Android media probe

Validated on 2026-11-18, physical Poco X8 Pro, Android 16/API 36, arm64.
This is a separate technical app (`io.dagaz.probe.media`), not the product UI.

## Versions and setup

- JDK 21, Gradle 8.13 wrapper, AGP 8.13.2, Kotlin/Compose plugin 2.2.20.
- Compose BOM 2025.08.00, Activity Compose 1.10.1, Media3 1.8.0.
- Compile/target SDK 36, minimum SDK 28. Only API 36 has been device-tested.
- SDK, JDK and Gradle downloads are local development tools, excluded from Git.
- Set `JAVA_HOME` to JDK 21 and `ANDROID_HOME` to your SDK, or create ignored
  `experiments/android-media/local.properties` with `sdk.dir` (escape a Windows
  drive colon, for example `C\:/Android/sdk`). Authorize USB debugging on the phone.

## Reproduction

In one terminal, from the repository root:

```powershell
node experiments/media-server/server.mjs
```

In another, with one authorized device attached:

```powershell
./experiments/android-media/run-device.ps1
# Unlock the phone before this separate, manually assisted phase:
./experiments/android-media/run-device.ps1 -Phase ScreenOff
```

The runner defaults to the repository-local ignored SDK's ADB. Use `-Adb` to
select another installation. It builds and installs only the probe package and
test package, then changes only its own `tcp:18888` reverse mapping. It does not
change Wi-Fi, airplane mode, battery settings, or access personal audio files.
During ScreenOff press power once when the tone starts and keep the screen off
until sound stops. The test intentionally pauses playback after 15 seconds.

Do not use Gradle's connected-test task between the two cache phases: on this
setup its UTP runner uninstalls both APKs afterwards, deleting the downloaded
fixture. Direct `am instrument` preserves data; the runner checks JUnit output
because the shell exit code alone does not report assertion failures reliably.

## Observed results

All three instrumented cases passed individually:

1. `MediaProbeTest`: PCM WAV, VBR MP3 and FLAC assets play; throttled network WAV
   starts before all 2,646,044 bytes are cached; explicit download completes
   despite a forced HTTP disconnect; seek, Activity recreation and stopped
   Activity retain playback and advancing position. Download and playback use
   the same logical cache key.
2. `OfflineProbeTest`: after force-stop and removal of server access, the full
   cached file remains available, playback resumes after reopening, and seeking
   to 25 seconds works. The test first verifies that HTTP cannot be reached.
3. `LockedScreenProbeTest`: physical power-button assistance, playback and
   position advance for 15 seconds; screen is off at the final assertion.
   The user also confirmed continuous audible playback while switching the
   screen off/on. This is not a measurement of every audio sample or continuous
   screen-state history.

An earlier two-second screen test sought to five seconds immediately after
locking. The user heard a brief interruption then; after removing that seek,
the interruption did not recur. This supports a test-induced seek as the cause,
without claiming a general absence of audio glitches.

`assembleDebug`, `assembleDebugAndroidTest`, and `lintDebug` passed. Lint retains
warnings about pinned dependency updates, prototype icon/backup configuration,
and the intentionally exported Media3 session service. The Node fixture helper's
three tests cover Range/If-Range and resumable failure injection.

## Limits and remaining checks

The prototype uses an unbounded shared download cache for one synthetic file.
It does not establish the product's disk budget, eviction, download snapshots,
revision validation, ownership handoff, or recovery after a server revision change.
Those belong to the planned client/server implementation.

Long screen-off playback, Doze/OEM battery restrictions, headset media buttons,
audio-focus loss, unplugging headphones, notification controls, minimum API 28,
and ordinary OS process eviction have not been validated. Force-stop recovery
does not promise automatic restart of a force-stopped app. P3 remains partial
until the relevant manual checks are completed.
