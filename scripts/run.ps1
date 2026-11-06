$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$plugins = Join-Path $root "plugins"
$hasPluginJars = (Get-ChildItem $plugins -Filter *.jar -File -ErrorAction SilentlyContinue).Count -gt 0
$bridgeClass = Join-Path $root "java\bridge\classes\rs\folia\bridge\Bridge.class"

if ($hasPluginJars -and !(Test-Path $bridgeClass)) {
    & (Join-Path $PSScriptRoot "build-java.ps1")
}

cargo run
