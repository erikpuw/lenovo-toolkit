$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $toml = Get-Content (Join-Path $root 'crates\lenovo-toolkit\Cargo.toml') -Raw
    if ($toml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
        throw 'Cannot read version from crates\lenovo-toolkit\Cargo.toml'
    }
    $ver = $Matches[1]

    Write-Host "Building lenovo-toolkit v$ver (release)..."
    cargo build --release
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "Building lenovo-toolkit-setup v$ver (release)..."
    cargo build --release -p lenovo-toolkit-setup
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null

    $portable = Join-Path $dist "lenovo-toolkit-v$ver-portable-x64.exe"
    $setup = Join-Path $dist "lenovo-toolkit-setup-v$ver-x64.exe"
    $cli = Join-Path $dist "lenovo-toolkit-cli-v$ver-x64.exe"
    Copy-Item -Force (Join-Path $root 'target\release\lenovo-toolkit.exe') $portable
    Copy-Item -Force (Join-Path $root 'target\release\lenovo-toolkit-setup.exe') $setup
    Copy-Item -Force (Join-Path $root 'target\release\lenovo-toolkit-cli.exe') $cli

    Get-Item $setup, $portable, $cli |
        Select-Object Name, Length, @{Name='KB';Expression={[math]::Round($_.Length/1024,1)}} |
        Format-Table -AutoSize
} finally {
    Pop-Location
}
