# Run only the isolated loopback RustFS probe. Credentials stay in ignored .env.test.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Push-Location $root
try {
    $file = Join-Path $root 'deploy/.env.test'
    if (-not (Test-Path -LiteralPath $file)) {
        $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
        $access = New-Object byte[] 10
        $secret = New-Object byte[] 32
        $rng.GetBytes($access)
        $rng.GetBytes($secret)
        $rng.Dispose()
        $key = ([BitConverter]::ToString($access)).Replace('-','').ToLowerInvariant()
        @("DAGAZ_TEST_S3_ACCESS_KEY=$key", "DAGAZ_TEST_S3_SECRET_KEY=$([Convert]::ToBase64String($secret))") | Set-Content -LiteralPath $file -Encoding ASCII
    }
    git check-ignore --quiet deploy/.env.test
    if ($LASTEXITCODE -ne 0) { throw 'Test credentials must be ignored by Git' }
    $settings = Get-Content -LiteralPath $file -Raw | ConvertFrom-StringData
    if ($settings.DAGAZ_TEST_S3_ACCESS_KEY -notmatch '^[a-f0-9]{20}$') { throw 'Test access key must use 20 hex characters (no SigV4 scope separator)' }
    if ($settings.DAGAZ_TEST_S3_SECRET_KEY.Length -lt 32) { throw 'Test secret is missing or too short' }
    docker compose --env-file deploy/.env.test -f deploy/compose.test.yaml up -d
    if ($LASTEXITCODE -ne 0) { throw 'RustFS test service failed to start' }
    $address = docker compose --env-file deploy/.env.test -f deploy/compose.test.yaml port rustfs 9000
    if ($LASTEXITCODE -ne 0 -or $address -notmatch '^127\.0\.0\.1:\d+$') { throw 'Test service must use a loopback port' }
    $env:DAGAZ_TEST_S3_ENDPOINT = "http://$address"
    $env:DAGAZ_TEST_S3_ACCESS_KEY = $settings.DAGAZ_TEST_S3_ACCESS_KEY
    $env:DAGAZ_TEST_S3_SECRET_KEY = $settings.DAGAZ_TEST_S3_SECRET_KEY
    $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
    if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
    & $cargo test --manifest-path experiments/storage/Cargo.toml --test s3
    if ($LASTEXITCODE -ne 0) { throw 'S3 probe failed' }
} finally { Pop-Location }
