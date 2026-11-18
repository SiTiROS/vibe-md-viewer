# Licenses of the third-party libraries included in the exe (build-only dependencies are not):
# dist\THIRD-PARTY-NOTICES.txt — goes into the zip and the release (MIT/Apache require the
# license text to accompany distributed copies).
# Usage: powershell -ExecutionPolicy Bypass -File scripts\notices.ps1
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$meta = cargo metadata --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
# Only what ends up in the exe: normal dependencies for Windows x64.
$used = cargo tree -e normal --target x86_64-pc-windows-msvc --prefix none -f '{p}' --locked |
    ForEach-Object { ($_ -replace '\s*\(\*\)$', '' -replace '\s*\(proc-macro\)$', '').Trim() } |
    Where-Object { $_ } | Sort-Object -Unique

$sb = New-Object Text.StringBuilder
[void]$sb.AppendLine('Third-party licenses of libraries included in vibe-md-viewer')
[void]$sb.AppendLine('')
$self = 'vibe-md-viewer'
$pkgs = $meta.packages | Where-Object { $used -contains "$($_.name) v$($_.version)" -and $_.name -ne $self } | Sort-Object name, version
foreach ($p in $pkgs) {
    [void]$sb.AppendLine(('=' * 78))
    [void]$sb.AppendLine("$($p.name) $($p.version) — $($p.license)")
    if ($p.repository) { [void]$sb.AppendLine($p.repository) }
    [void]$sb.AppendLine(('=' * 78))
    $dir = Split-Path -Parent $p.manifest_path
    $files = Get-ChildItem $dir -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE)' } | Sort-Object Name
    if (-not $files) {
        [void]$sb.AppendLine("(no license file shipped with the package; license: $($p.license), text — https://spdx.org/licenses/)")
    }
    foreach ($f in $files) {
        [void]$sb.AppendLine("--- $($f.Name)")
        [void]$sb.AppendLine([IO.File]::ReadAllText($f.FullName).Trim())
        [void]$sb.AppendLine('')
    }
    [void]$sb.AppendLine('')
}
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
$out = Join-Path $dist 'THIRD-PARTY-NOTICES.txt'
[IO.File]::WriteAllText($out, $sb.ToString(), (New-Object Text.UTF8Encoding $true))
"Licenses: dist\THIRD-PARTY-NOTICES.txt ($($pkgs.Count) packages)"
