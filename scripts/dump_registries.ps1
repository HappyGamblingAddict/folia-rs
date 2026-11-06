#requires -Version 5.1
[CmdletBinding()]
param()
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$assetDir = Join-Path $root "crates/folia-core/assets/registries"
New-Item -ItemType Directory -Force -Path $assetDir | Out-Null

Write-Host "Dumping dynamic registries from assets/join_config.bin..."
$lines = & cargo run -q -p folia-core --example dump_registries
if ($LASTEXITCODE -ne 0) {
    throw "dump_registries failed with exit code $LASTEXITCODE"
}

$registries = @{}
foreach ($line in $lines) {
    $parts = $line -split "`t"
    if ($parts.Count -lt 4 -or $parts[0] -ne "REG") { continue }
    $registry = $parts[1]
    $id = [int]$parts[2]
    $name = $parts[3]
    if (-not $registries.ContainsKey($registry)) {
        $registries[$registry] = New-Object System.Collections.Generic.List[string]
    }
    $registries[$registry].Add("$id`t$name")
}

foreach ($registry in ($registries.Keys | Sort-Object)) {
    $file = ($registry -replace "^minecraft:", "") -replace "[/\\]", "_"
    $file = "$file.tsv"
    $path = Join-Path $assetDir $file
    $content = $registries[$registry] | Sort-Object { [int](($_ -split "`t")[0]) }
    [System.IO.File]::WriteAllLines($path, [string[]]$content, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host ("  {0,-32} {1,4} entries -> {2}" -f $registry, $content.Count, $file)
}

Write-Host "Done: $($registries.Count) registry tables in $assetDir"
