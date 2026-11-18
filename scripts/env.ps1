# Build environment: everything the tools normally put into the user profile on C:
# is redirected into the project folder (.toolcache). Use via dot-source:
#   . .\scripts\env.ps1
$root = Split-Path -Parent $PSScriptRoot
$cache = Join-Path $root '.toolcache'
$env:CARGO_HOME = Join-Path $cache 'cargo'          # crate cache and the crates.io registry
$env:CARGO_TARGET_DIR = Join-Path $root 'target'     # build artifacts
$env:TEMP = Join-Path $cache 'tmp'                   # rustc/link.exe temporary files
$env:TMP = $env:TEMP
foreach ($d in @($env:CARGO_HOME, $env:TEMP)) {
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Force $d | Out-Null }
}
# The Rust toolchain itself is already installed in the user profile; only its bin folder is
# added to PATH, nothing is downloaded into it.
$rustBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (($env:PATH -split ';') -notcontains $rustBin) { $env:PATH = "$rustBin;$env:PATH" }
