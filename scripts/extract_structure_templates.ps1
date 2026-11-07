#requires -Version 5.1
<#
.SYNOPSIS
    Builds the embedded vanilla structure-template catalog.

.DESCRIPTION
    Reads the decompiled 1.21.11 structure NBT tree (../decompiled by default)
    and writes a deterministic JSON catalog:

        crates/folia-core/assets/structure_templates.json

    Every entry is keyed by its normalized structure id
    (`minecraft:village/plains/town_centers/plains_meeting_point_1`) and its
    value is the base64 encoding of the template's gzip-compressed NBT bytes.
    Ids and object keys are ordinal-sorted, there is no insignificant
    whitespace, and the file ends with one LF, so repeated runs are
    byte-identical.

    The default `-TemplateRoot village` keeps the asset below the 2.5 MB budget
    while covering every village pool template. Pass another root only when the
    asset budget allows it; the script refuses catalogs above 2.5 MB.

.PARAMETER SourceRoot
    Directory containing the `structure` tree, usually
    `<decompiled>/data/minecraft/structure`. Defaults to the adjacent
    decompiled checkout or the known local path.

.PARAMETER OutputPath
    Catalog path. Defaults to
    crates/folia-core/assets/structure_templates.json.

.PARAMETER TemplateRoot
    Subdirectory of the structure tree to catalog. Defaults to `village`.

.PARAMETER Check
    Generate in memory and compare with the existing catalog; do not write.
#>
[CmdletBinding()]
param(
    [string]$SourceRoot = "",
    [string]$OutputPath = "",
    [string]$TemplateRoot = "village",
    [switch]$Check
)

$ErrorActionPreference = "Stop"
$invariant = [System.Globalization.CultureInfo]::InvariantCulture
$repoRoot = Split-Path -Parent $PSScriptRoot

if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "crates/folia-core/assets/structure_templates.json"
}

function Find-StructureRoot {
    $candidates = @(
        $SourceRoot,
        (Join-Path (Split-Path -Parent $repoRoot) "decompiled/data/minecraft/structure"),
        (Join-Path (Split-Path -Parent $repoRoot) "decompiled/data/minecraft/structure/village"),
        "C:\Users\austr\Downloads\rustfol\decompiled\data\minecraft\structure"
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Container)) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return ""
}

if (-not $SourceRoot) {
    $SourceRoot = Find-StructureRoot
}
if (-not $SourceRoot) {
    throw "Cannot find decompiled/data/minecraft/structure; pass -SourceRoot <path>."
}
$SourceRoot = (Resolve-Path -LiteralPath $SourceRoot).Path

if ($TemplateRoot) {
    $root = Join-Path $SourceRoot $TemplateRoot
} else {
    $root = $SourceRoot
}
if (-not (Test-Path -LiteralPath $root -PathType Container)) {
    throw "Template root '$TemplateRoot' does not exist under $SourceRoot."
}

function Write-JsonString {
    param([string]$Text, [System.Text.StringBuilder]$Builder)

    [void]$Builder.Append('"')
    foreach ($character in $Text.ToCharArray()) {
        $code = [int]$character
        switch ($code) {
            8 { [void]$Builder.Append('\b') }
            9 { [void]$Builder.Append('\t') }
            10 { [void]$Builder.Append('\n') }
            12 { [void]$Builder.Append('\f') }
            13 { [void]$Builder.Append('\r') }
            34 { [void]$Builder.Append('\"') }
            92 { [void]$Builder.Append('\\') }
            default {
                if ($code -lt 32) {
                    [void]$Builder.AppendFormat('\u{0:x4}', $code)
                } else {
                    [void]$Builder.Append($character)
                }
            }
        }
    }
    [void]$Builder.Append('"')
}

function Get-Sha256([byte[]]$Bytes) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace("-", "").ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

$files = @(Get-ChildItem -LiteralPath $root -Recurse -File -Filter "*.nbt")
$paths = [string[]]@($files | ForEach-Object { $_.FullName })
[Array]::Sort($paths, [System.StringComparer]::Ordinal)

$entries = [System.Collections.Generic.List[string]]::new()
$byRoot = @{}
$totalBytes = 0
$seen = @{}

foreach ($path in $paths) {
    $relative = $path.Substring($SourceRoot.Length + 1).Replace('\', '/')
    if (-not $relative.EndsWith(".nbt", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Unexpected non-NBT file in structure tree: $relative"
    }
    $id = "minecraft:" + $relative.Substring(0, $relative.Length - 4)
    if ($seen.ContainsKey($id)) {
        throw "Duplicate structure template id: $id"
    }
    $seen[$id] = $true

    $bytes = [System.IO.File]::ReadAllBytes($path)
    if ($bytes.Length -lt 18 -or $bytes[0] -ne 0x1f -or $bytes[1] -ne 0x8b) {
        throw "Template $id is not gzip-compressed NBT."
    }

    if ($TemplateRoot) {
        $withinRoot = $relative.Substring($TemplateRoot.Length + 1)
    } else {
        $withinRoot = $relative
    }
    $slash = $withinRoot.IndexOf('/')
    if ($slash -lt 0) {
        $category = "(root)"
    } else {
        $category = $withinRoot.Substring(0, $slash)
    }
    if (-not $byRoot.ContainsKey($category)) {
        $byRoot[$category] = 0
    }
    $byRoot[$category] = $byRoot[$category] + 1

    $entries.Add($id)
    $totalBytes += $bytes.Length
    $entries.Add([System.Convert]::ToBase64String($bytes))
}

$categories = [string[]]@($byRoot.Keys)
[Array]::Sort($categories, [System.StringComparer]::Ordinal)

$builder = [System.Text.StringBuilder]::new(2MB)
[void]$builder.Append('{"counts":{"by_root":{')
for ($index = 0; $index -lt $categories.Length; $index++) {
    if ($index -gt 0) {
        [void]$builder.Append(',')
    }
    Write-JsonString -Text $categories[$index] -Builder $builder
    [void]$builder.Append(':')
    [void]$builder.Append([System.Convert]::ToString($byRoot[$categories[$index]], $invariant))
}
[void]$builder.Append('},"templates":')
[void]$builder.Append([System.Convert]::ToString($seen.Count, $invariant))
[void]$builder.Append('},"data_version":4788,"templates":{')
for ($index = 0; $index -lt $entries.Count; $index += 2) {
    if ($index -gt 0) {
        [void]$builder.Append(',')
    }
    Write-JsonString -Text $entries[$index] -Builder $builder
    [void]$builder.Append(':"')
    [void]$builder.Append($entries[$index + 1])
    [void]$builder.Append('"')
}
[void]$builder.Append('},"total_bytes":')
[void]$builder.Append([System.Convert]::ToString($totalBytes, $invariant))
[void]$builder.Append(',"version":"1.21.11"}')
[void]$builder.Append("`n")

$json = $builder.ToString()
$utf8 = New-Object System.Text.UTF8Encoding($false)
$outputBytes = $utf8.GetBytes($json)
$sha256 = Get-Sha256 $outputBytes

if ($outputBytes.Length -gt 2500000) {
    throw ("Catalog is {0:N0} bytes; the structure-template asset budget is 2.5 MB." -f $outputBytes.Length)
}

if ($Check) {
    if (-not (Test-Path -LiteralPath $OutputPath -PathType Leaf)) {
        throw "Catalog $OutputPath does not exist; run without -Check to generate it."
    }
    $existing = [System.IO.File]::ReadAllBytes($OutputPath)
    if (-not [System.Linq.Enumerable]::SequenceEqual($existing, $outputBytes)) {
        throw ("Catalog is stale: $OutputPath does not match the generated bytes (generated sha256 $sha256).")
    }
    Write-Host ("OK {0} ({1:N0} templates, {2:N0} bytes, sha256 {3})" -f $OutputPath, $seen.Count, $outputBytes.Length, $sha256)
    return
}

[System.IO.File]::WriteAllBytes($OutputPath, $outputBytes)
Write-Host ("Wrote {0} ({1:N0} templates, {2:N0} bytes, {3:N0} source bytes, sha256 {4})" -f $OutputPath, $seen.Count, $outputBytes.Length, $totalBytes, $sha256)
