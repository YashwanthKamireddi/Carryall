# Carryall — zero-manual install for Windows (no Rust needed). Run in PowerShell:
#   irm https://raw.githubusercontent.com/YashwanthKamireddi/Carryall/main/install.ps1 | iex
$ErrorActionPreference = "Stop"
$repo  = "YashwanthKamireddi/Carryall"
$name  = "windows-x86_64"   # runs natively on x64 and via emulation on arm64
$asset = "carryall-$name.zip"
$url   = "https://github.com/$repo/releases/latest/download/$asset"
$dir   = Join-Path $env:LOCALAPPDATA "Carryall"

New-Item -ItemType Directory -Force -Path $dir | Out-Null
$zip = Join-Path $env:TEMP $asset
Write-Host "Downloading $asset…"
Invoke-WebRequest -Uri $url -OutFile $zip
$ext = Join-Path $env:TEMP "carryall-$name"
Expand-Archive -Path $zip -DestinationPath $env:TEMP -Force
Copy-Item (Join-Path $ext "carryall.exe"), (Join-Path $ext "carryall-relay.exe") $dir -Force
Write-Host "Installed to $dir"

# First-run setup: generates a key, writes config, installs autostart.
& (Join-Path $dir "carryall.exe") setup -y
Write-Host "Done. Pair your phone with:  $dir\carryall.exe pair"
