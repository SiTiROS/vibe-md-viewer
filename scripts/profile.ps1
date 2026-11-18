# Measures heavy operations on a large document: opening, theme/width/zoom changes,
# mode switches, scrolling. The document (~275 KB, 140 entries with code) is assembled
# from tests\fixtures\inline-code.md into .toolcache\t\big.md.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\profile.ps1 [-Exe dist\vibe-md-viewer.exe] [-Size 2560x1380]
param([string]$Exe = "dist\vibe-md-viewer.exe", [int]$Copies = 70, [string]$Size = "1700x1000")
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$dir = Join-Path $root '.toolcache\t'
New-Item -ItemType Directory -Force $dir | Out-Null
$src = [IO.File]::ReadAllText((Join-Path $root 'tests\fixtures\inline-code.md'), [Text.Encoding]::UTF8)
$body = $src.Substring($src.IndexOf("## 128."))
$sb = New-Object Text.StringBuilder
[void]$sb.Append("# Big report`n`n| Severity | Total |`n| --- | ---: |`n| Critical | 12 |`n`n")
for ($i = 0; $i -lt $Copies; $i++) {
    [void]$sb.Append($body.Replace("## 128.", "## $(128 + 2*$i).").Replace("## 129.", "## $(129 + 2*$i)."))
    [void]$sb.Append("`n")
}
$md = Join-Path $dir 'big.md'
[IO.File]::WriteAllText($md, $sb.ToString(), (New-Object Text.UTF8Encoding $false))
$log = Join-Path $dir 'profile.txt'
Remove-Item $log -ErrorAction SilentlyContinue
$p = Start-Process -FilePath $Exe -ArgumentList @("`"$md`"", '--profile', "`"$log`"", '--size', $Size) -PassThru
if (-not $p.WaitForExit(120000)) { $p.Kill(); "TIMEOUT"; exit 1 }
"Document: {0:N0} KB" -f ((Get-Item $md).Length / 1KB)
Get-Content $log -Encoding UTF8
