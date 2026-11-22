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

    $sizeEn = ([math]::Round((Get-Item $setup).Length / 1MB, 1)).ToString('0.0', [System.Globalization.CultureInfo]::InvariantCulture)
    $sizeVi = $sizeEn.Replace('.', ',')
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)

    $readmeViPath = Join-Path $root 'README.md'
    if (Test-Path $readmeViPath) {
        $text = [System.IO.File]::ReadAllText($readmeViPath, $utf8NoBom)
        $text = [regex]::Replace($text, 'Bản mới nhất: v\d+\.\d+\.\d+', "Bản mới nhất: v$ver")
        $text = [regex]::Replace($text, 'lenovo-toolkit-setup-v\d+\.\d+\.\d+-x64\.exe', "lenovo-toolkit-setup-v$ver-x64.exe")
        $text = [regex]::Replace($text, 'khoảng \d+(?:,\d+)? MB', "khoảng $sizeVi MB")
        [System.IO.File]::WriteAllText($readmeViPath, $text, $utf8NoBom)
    }

    $readmeEnPath = Join-Path $root 'README.en.md'
    if (Test-Path $readmeEnPath) {
        $text = [System.IO.File]::ReadAllText($readmeEnPath, $utf8NoBom)
        $text = [regex]::Replace($text, 'Latest: v\d+\.\d+\.\d+', "Latest: v$ver")
        $text = [regex]::Replace($text, 'lenovo-toolkit-setup-v\d+\.\d+\.\d+-x64\.exe', "lenovo-toolkit-setup-v$ver-x64.exe")
        $text = [regex]::Replace($text, 'about \d+(?:\.\d+)? MB', "about $sizeEn MB")
        [System.IO.File]::WriteAllText($readmeEnPath, $text, $utf8NoBom)
    }

    Get-Item $setup, $portable, $cli |
        Select-Object Name, Length, @{Name='KB';Expression={[math]::Round($_.Length/1024,1)}} |
        Format-Table -AutoSize
} finally {
    Pop-Location
}
