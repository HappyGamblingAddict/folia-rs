#requires -Version 5.1
[CmdletBinding()]
param()
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$source = "C:\Users\austr\Downloads\rustfol\decompiled\data\minecraft\worldgen\biome"
$output = "crates/folia-core/assets/biome_climate.tsv"
$rows = New-Object System.Collections.Generic.List[string]

foreach ($file in (Get-ChildItem $source -Filter "*.json" | Sort-Object Name)) {
    $json = Get-Content $file.FullName -Raw | ConvertFrom-Json
    $name = "minecraft:" + $file.BaseName
    $temperature = $json.temperature
    $modifier = if ($json.temperature_modifier) { $json.temperature_modifier } else { "none" }
    $rows.Add("$name`t$temperature`t$modifier")
}

[System.IO.File]::WriteAllLines((Join-Path $root $output), [string[]]$rows, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "Wrote $($rows.Count) biome climate rows to $output"
