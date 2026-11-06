#requires -Version 5.1
[CmdletBinding()]
param(
    [string]$Label = "snapshot"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$safeLabel = $Label -replace '[^A-Za-z0-9._-]', '-'
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$snapshotDir = Join-Path $root "snapshots"
New-Item -ItemType Directory -Path $snapshotDir -Force | Out-Null

$zipPath = Join-Path $snapshotDir "folia-rs-$stamp-$safeLabel.zip"

$items = @(
    "AGENTS.md",
    "BLOCKERS.md",
    "Cargo.lock",
    "Cargo.toml",
    "DECISIONS.md",
    "HANDOFF.md",
    "PORTING.md",
    "PORT_MAP.md",
    "README.md",
    "STATE.md",
    "TODO.md",
    "crates",
    "java",
    "plugins",
    "scripts",
    "server.properties",
    "world"
) | Where-Object { Test-Path $_ }

if ($items.Count -eq 0) {
    throw "No snapshot items found in $root"
}

Write-Host "Creating snapshot:"
Write-Host "  $zipPath"
Write-Host "Including:"
foreach ($item in $items) {
    Write-Host "  $item"
}

Compress-Archive -Path $items -DestinationPath $zipPath -CompressionLevel Optimal

$size = (Get-Item $zipPath).Length
Write-Host ("Snapshot complete: {0:N1} MB" -f ($size / 1MB)) -ForegroundColor Green
