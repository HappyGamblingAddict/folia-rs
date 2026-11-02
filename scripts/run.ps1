$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (!(Test-Path (Join-Path $root "java\bridge\classes\rs\folia\bridge\Bridge.class"))) {
    & (Join-Path $PSScriptRoot "build-java.ps1")
}

cargo run
