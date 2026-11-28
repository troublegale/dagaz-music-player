param([ValidateSet('Pairing','RealServer','ServerPlayback','ServerOffline','Offline','Playback','ScreenLock','Headset')][string]$Phase='Pairing')
$ErrorActionPreference='Stop'
$repoRoot=(Resolve-Path -LiteralPath "$PSScriptRoot/../..").Path
$adb=(Resolve-Path -LiteralPath "$repoRoot/.tools/android/platform-tools/adb.exe").Path
$fixture=(Resolve-Path -LiteralPath "$repoRoot/.superpowers/sdd/2026-11-18-dagaz-music-player-plan").Path
function Invoke-Adb {
    & $adb @args
    if($LASTEXITCODE -ne 0){throw 'ADB command failed'}
}
function Invoke-Tests([string]$Classes,[int]$Count,[hashtable]$Extra=@{}) {
    $instrumentArgs=@('shell','am','instrument','-w','-r','-e','class',$Classes)
    foreach($entry in $Extra.GetEnumerator()){$instrumentArgs+=@('-e',$entry.Key,$entry.Value)}
    $instrumentArgs+='io.dagaz.player.test/androidx.test.runner.AndroidJUnitRunner'
    $result=Invoke-Adb @instrumentArgs
    $result | Set-Content -LiteralPath "$fixture/android-$Phase-instrumentation.log"
    if($Classes -match '#([A-Za-z0-9_]+)$'){$result | Set-Content -LiteralPath "$fixture/android-$Phase-$($Matches[1])-instrumentation.log"}
    $result | Out-Host
    if(($result -join "`n") -match 'INSTRUMENTATION_STATUS_CODE: -(3|4)\b'){throw 'Android tests were skipped; the existing personal connection was preserved'}
    if(($result -join "`n") -notmatch "OK \($Count tests?\)"){throw 'Android instrumentation did not pass'}
}
& "$PSScriptRoot/gradlew.bat" -p $PSScriptRoot testDebugUnitTest assembleDebug assembleDebugAndroidTest lintDebug
if($LASTEXITCODE -ne 0){throw 'Android build/tests/lint failed'}
# Push-install succeeds on this Poco where ADB's streaming installer is rejected without a prompt.
Invoke-Adb install --no-streaming -r -t "$PSScriptRoot/app/build/outputs/apk/debug/app-debug.apk"
Invoke-Adb install --no-streaming -r -t "$PSScriptRoot/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
if($Phase -eq 'Pairing') {
    Invoke-Tests 'io.dagaz.player.PairingTest,io.dagaz.player.ConnectionScreenTest' 2
    return
}
if($Phase -eq 'Playback') {
    Invoke-Tests 'io.dagaz.player.PlaybackServiceTest' 9
    return
}
if($Phase -eq 'Offline'){Invoke-Tests 'io.dagaz.player.OfflineTest' 5;return}
if($Phase -in @('ScreenLock','Headset')) {
    $manual=if($Phase -eq 'ScreenLock'){'lock'}else{'headset'}
    Invoke-Tests 'io.dagaz.player.PlaybackServiceTest#libraryAndBackgroundPlayback' 1 @{manual=$manual}
    return
}
$env:DAGAZ_ANDROID_FIXTURE_DIR=$fixture
$ready=Join-Path $fixture 'android-fixture.json'
$finished=Join-Path $fixture 'android-fixture.finished'
foreach($file in @($ready,$finished)){if(Test-Path -LiteralPath $file){Remove-Item -LiteralPath $file}}
$cargoCommand=Get-Command cargo.exe -ErrorAction SilentlyContinue
$cargo=if($cargoCommand){$cargoCommand.Source}else{Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe'}
if(!(Test-Path -LiteralPath $cargo)){throw 'Rust Cargo is required for the real-server fixture'}
& $cargo test -p dagaz-server --test android_pairing --no-run
if($LASTEXITCODE -ne 0){throw 'Rust Android fixture build failed'}
$previousMediaFlag=$env:DAGAZ_ANDROID_MEDIA
try {
    if($Phase -in @('ServerPlayback','ServerOffline')){$env:DAGAZ_ANDROID_MEDIA='1'}else{Remove-Item Env:DAGAZ_ANDROID_MEDIA -ErrorAction SilentlyContinue}
    $process=Start-Process -FilePath $cargo -ArgumentList @('test','-p','dagaz-server','--test','android_pairing','--','--ignored','--nocapture') -WorkingDirectory $repoRoot -WindowStyle Hidden -RedirectStandardOutput "$fixture/a1-server-fixture.log" -RedirectStandardError "$fixture/a1-server-fixture-build.log" -PassThru
}finally{$env:DAGAZ_ANDROID_MEDIA=$previousMediaFlag}
$timer=[Diagnostics.Stopwatch]::StartNew()
while(!(Test-Path -LiteralPath $ready)) {
    if($process.HasExited){throw 'Rust Android fixture failed; see ignored fixture logs'}
    if($timer.Elapsed.TotalSeconds -gt 120){throw 'Rust Android fixture did not become ready'}
    Start-Sleep -Milliseconds 200
}
$settings=Get-Content -LiteralPath $ready -Raw | ConvertFrom-Json
$serverUri=[Uri]$settings.url
if($serverUri.Host -ne '127.0.0.1'){throw 'Fixture must be loopback'}
$port=$serverUri.Port
$passed=$false
Invoke-Adb reverse "tcp:$port" "tcp:$port"
$linked=$true
try {
    if($Phase -eq 'ServerOffline'){
        $nonce=[guid]::NewGuid().ToString()
        Invoke-Tests 'io.dagaz.player.ServerOfflineTest#downloadFixture' 1 @{server_url=$settings.url;pairing_code=$settings.code;album_id=$settings.album_id;playlist_id=$settings.playlist_id;fixture_nonce=$nonce}
        Invoke-Adb reverse --remove "tcp:$port"
        $linked=$false
        Invoke-Tests 'io.dagaz.player.ServerOfflineTest#coldStartWithoutServer' 1 @{server_url=$settings.url;fixture_nonce=$nonce}
    }elseif($Phase -eq 'ServerPlayback'){
        Invoke-Tests 'io.dagaz.player.ServerPlaybackTest' 1 @{server_url=$settings.url;pairing_code=$settings.code;track_id=$settings.track_id;alias_track_id=$settings.alias_track_id}
    }else{Invoke-Tests 'io.dagaz.player.ServerPairingTest' 1 @{server_url=$settings.url;pairing_code=$settings.code}}
    $passed=$true
} finally {
    Set-Content -LiteralPath $finished -Value $(if($passed){'ok'}else{'failed'})
    if($linked){Invoke-Adb reverse --remove "tcp:$port"}
}
if(!$process.WaitForExit(15000)){throw 'Rust fixture did not finish cleanup'}
if($process.ExitCode -ne 0){throw 'Rust fixture reported failure'}
