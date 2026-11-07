#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$Full
)

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$failures = New-Object System.Collections.Generic.List[string]

function Write-Header([string]$Text) {
    Write-Host ""
    Write-Host "=== $Text ===" -ForegroundColor Cyan
}

function Run-Step {
    param(
        [string]$Name,
        [scriptblock]$Command
    )

    Write-Header $Name
    $oldErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $Command 2>&1 | ForEach-Object { Write-Host $_ }
    $code = $LASTEXITCODE
    $ErrorActionPreference = $oldErrorAction
    if ($code -ne 0) {
        $failures.Add($Name)
        Write-Host "FAILED: $Name (exit $code)" -ForegroundColor Red
    } else {
        Write-Host "OK: $Name" -ForegroundColor Green
    }
}

Write-Header "Required assets"
$assets = @(
    "crates/folia-core/assets/biome_climate.tsv",
    "crates/folia-core/assets/biome_ids.txt",
    "crates/folia-core/assets/biome_params.txt",
    "crates/folia-core/assets/block_defaults.tsv",
    "crates/folia-core/assets/block_shapes.tsv.gz",
    "crates/folia-core/assets/block_state_behaviour.tsv.gz",
    "crates/folia-core/assets/block_states.tsv",
    "crates/folia-core/assets/entity_type_ids.tsv",
    "crates/folia-core/assets/feature_data.json",
    "crates/folia-core/assets/item_ids.tsv",
    "crates/folia-core/assets/java-biome-samples.txt",
    "crates/folia-core/assets/java-chunk-0--1.txt",
    "crates/folia-core/assets/java-density-grid.txt",
    "crates/folia-core/assets/join_config.bin",
    "crates/folia-core/assets/join_play.bin",
    "crates/folia-core/assets/item_prototypes.json",
    "crates/folia-core/assets/loot_tables.json",
    "crates/folia-core/assets/mth_sin.bin",
    "crates/folia-core/assets/recipes.json",
    "crates/folia-core/assets/structure_processors.json",
    "crates/folia-core/assets/structure_templates.json",
    "crates/folia-core/assets/template_pools.json",
    "crates/folia-core/assets/tags/block.tsv",
    "crates/folia-core/assets/tags/damage_type.tsv",
    "crates/folia-core/assets/tags/enchantment.tsv",
    "crates/folia-core/assets/tags/entity_type.tsv",
    "crates/folia-core/assets/tags/fluid.tsv",
    "crates/folia-core/assets/tags/game_event.tsv",
    "crates/folia-core/assets/tags/item.tsv",
    "crates/folia-core/assets/tags/point_of_interest_type.tsv",
    "crates/folia-core/assets/tags/worldgen.tsv",
    "crates/folia-core/assets/worldgen.json"
)
foreach ($asset in $assets) {
    if (Test-Path $asset) {
        Write-Host "OK: $asset"
    } else {
        Write-Host "MISSING: $asset" -ForegroundColor Red
        $failures.Add("missing asset: $asset")
    }
}

$registryAssets = @(Get-ChildItem "crates/folia-core/assets/registries/*.tsv" -ErrorAction SilentlyContinue)
if ($registryAssets.Count -eq 23) {
    Write-Host "OK: 23 dynamic registry assets"
} else {
    Write-Host "MISSING: expected 23 dynamic registry assets, found $($registryAssets.Count)" -ForegroundColor Red
    $failures.Add("dynamic registry asset count")
}

Run-Step "cargo fmt --check" {
    cargo fmt --all -- --check
}

Run-Step "cargo clippy" {
    cargo clippy --workspace --all-targets -- -D warnings
}

Run-Step "cargo test --workspace" {
    cargo test --workspace --no-fail-fast
}

Run-Step "worldgen parity" {
    cargo test -p folia-core --test worldgen_parity -- --nocapture
}

if ($Full) {
    Run-Step "worldgen join" {
        cargo test -p folia-core --test worldgen_join -- --nocapture
    }

    Run-Step "release build" {
        cargo build --workspace --release
    }
}

Write-Header "Verification summary"
if ($failures.Count -eq 0) {
    Write-Host "All checks passed." -ForegroundColor Green
    exit 0
}

Write-Host "Failures:" -ForegroundColor Red
foreach ($failure in $failures) {
    Write-Host "  - $failure" -ForegroundColor Red
}
exit 1
