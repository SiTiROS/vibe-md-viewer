# Distribution package:
#   dist\vibe-md-viewer-<version>-setup.exe  — the installer (the same exe: a name with "setup" opens the wizard)
#   dist\vibe-md-viewer-<version>.zip         — setup.exe + README.txt + ПРОЧТИ.txt (Russian) + LICENSE + THIRD-PARTY-NOTICES.txt
#   dist\vibe-md-viewer-<version>.sha256.txt  — checksums
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

& (Join-Path $PSScriptRoot 'build.ps1') -Test
if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne $null) { throw "Build failed" }

$ver = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$dist = Join-Path $root 'dist'
# Only files of this same version are deleted: older installers stay (needed to test updating).
foreach ($f in @("vibe-md-viewer-$ver-setup.exe", "vibe-md-viewer-$ver.zip", "vibe-md-viewer-$ver.sha256.txt")) {
    $fp = Join-Path $dist $f
    if (Test-Path $fp) { Remove-Item $fp -Force }
}
$setup = Join-Path $dist "vibe-md-viewer-$ver-setup.exe"
Copy-Item (Join-Path $dist 'vibe-md-viewer.exe') $setup -Force

$stage = Join-Path $root ".toolcache\package\vibe-md-viewer-$ver"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $setup $stage
& (Join-Path $PSScriptRoot 'notices.ps1')
Copy-Item (Join-Path $dist 'THIRD-PARTY-NOTICES.txt') $stage
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $stage 'LICENSE.txt')
foreach ($p in @(@('README-dist.en.txt', 'README.txt'), @('README-dist.ru.txt', 'ПРОЧТИ.txt'))) {
    $readme = [IO.File]::ReadAllText((Join-Path $root "assets\$($p[0])"), [Text.Encoding]::UTF8).Replace('{{VERSION}}', $ver)
    [IO.File]::WriteAllText((Join-Path $stage $p[1]), $readme, (New-Object Text.UTF8Encoding $true))
}

$zip = Join-Path $dist "vibe-md-viewer-$ver.zip"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
$hz = (Get-FileHash $zip -Algorithm SHA256).Hash
$hs = (Get-FileHash $setup -Algorithm SHA256).Hash
Set-Content -Path (Join-Path $dist "vibe-md-viewer-$ver.sha256.txt") -Value "$hs  vibe-md-viewer-$ver-setup.exe`r`n$hz  vibe-md-viewer-$ver.zip" -Encoding ASCII
"Installer: dist\vibe-md-viewer-$ver-setup.exe ({0:N2} MB)" -f ((Get-Item $setup).Length / 1MB)
"Package:   dist\vibe-md-viewer-$ver.zip ({0:N2} MB)" -f ((Get-Item $zip).Length / 1MB)
"SHA-256 setup: $hs"
