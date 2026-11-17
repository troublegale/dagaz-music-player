param(
    [ValidateSet('NetworkOffline', 'ScreenOff')]
    [string]$Phase = 'NetworkOffline',
    [string]$Adb = "$PSScriptRoot/../../.tools/android/platform-tools/adb.exe"
)
$ErrorActionPreference = 'Stop'
$Adb = (Resolve-Path -LiteralPath $Adb).Path

function Invoke-Adb {
    & $Adb @args
    if ($LASTEXITCODE -ne 0) { throw "ADB failed: $args" }
}

function Invoke-Probe([string]$Class) {
    $result = Invoke-Adb shell am instrument -w -r -e class "io.dagaz.probe.media.$Class" io.dagaz.probe.media.test/androidx.test.runner.AndroidJUnitRunner
    $result | Out-Host
    # Android's instrumentation command may exit zero even when JUnit failed.
    if (($result -join "`n") -notmatch 'OK \(1 test\)') { throw "$Class did not pass" }
}

if ($Phase -eq 'ScreenOff') {
    Write-Host 'Unlock the phone first. During the tone press power once; keep the screen off until the tone ends (15 seconds).'
    Invoke-Probe 'LockedScreenProbeTest'
    return
}

& "$PSScriptRoot/gradlew.bat" -p $PSScriptRoot assembleDebug assembleDebugAndroidTest lintDebug
if ($LASTEXITCODE -ne 0) { throw 'Android build/lint failed' }
Invoke-Adb install -r -t "$PSScriptRoot/app/build/outputs/apk/debug/app-debug.apk"
Invoke-Adb install -r -t "$PSScriptRoot/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
# Start experiments/media-server/server.mjs separately before this phase.
Invoke-Adb reverse tcp:18888 tcp:18888
try {
    Invoke-Probe 'MediaProbeTest'
} finally {
    Invoke-Adb reverse --remove tcp:18888
}
# Do not uninstall/reinstall here: the second test needs the first test's cache.
Invoke-Adb shell am force-stop io.dagaz.probe.media
Invoke-Probe 'OfflineProbeTest'
