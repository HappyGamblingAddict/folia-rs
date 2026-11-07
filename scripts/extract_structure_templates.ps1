#requires -Version 5.1
<#
.SYNOPSIS
    Builds the embedded vanilla structure-template catalogs.

.DESCRIPTION
    Reads the decompiled 1.21.11 structure NBT tree (../decompiled by default)
    and writes two deterministic JSON catalogs:

        crates/folia-core/assets/structure_templates.json
        crates/folia-core/assets/structure_templates_more.json

    The first catalog contains every template under `village/`. Its entries are
    keyed by normalized structure id and each value is the base64 encoding of
    the template's original gzip-compressed NBT bytes.

    The second catalog contains the remaining template families. A 719-entry
    per-template catalog would exceed the 2.5 MB budget, so it is stored as one
    base64+gzip NBT stream per top-level family plus an offset/length index:

        "blobs":     { "<family>": { "bytes": <plain NBT bytes>, "payload": "..." } }
        "templates": { "<id>": { "blob": "<family>", "length": N, "offset": N } }

    The family streams are concatenations of the original NBT bytes, so every
    indexed slice is a complete gzip-decoded NBT document. `trial_chambers` is
    deliberately omitted: its 191 templates alone occupy about 1.0 MB in
    base64+gzip form, and including it would push the catalog past 2.5 MB.

    Ids and object keys are ordinal-sorted, there is no insignificant
    whitespace, and each file ends with one LF, so repeated runs are
    byte-identical. The more catalog is recompressed with Java's zlib
    `Deflater` level 9; that dependency is required only for the second file.

.PARAMETER SourceRoot
    Directory containing the `structure` tree, usually
    `<decompiled>/data/minecraft/structure`. Defaults to the adjacent
    decompiled checkout or the known local path.

.PARAMETER OutputPath
    Village catalog path. Defaults to
    crates/folia-core/assets/structure_templates.json.

.PARAMETER OutputPathMore
    Non-village catalog path. Defaults to
    crates/folia-core/assets/structure_templates_more.json.

.PARAMETER TemplateRoot
    Subdirectory of the structure tree for the first catalog. Defaults to
    `village`.

.PARAMETER ExcludeRoot
    Additional top-level subdirectory omitted from the second catalog.
    Defaults to `trial_chambers`.

.PARAMETER Check
    Generate in memory and compare with the existing catalogs; do not write.
#>
[CmdletBinding()]
param(
    [string]$SourceRoot = "",
    [string]$OutputPath = "",
    [string]$OutputPathMore = "",
    [string]$TemplateRoot = "village",
    [string]$ExcludeRoot = "trial_chambers",
    [switch]$Check
)

$ErrorActionPreference = "Stop"
$invariant = [System.Globalization.CultureInfo]::InvariantCulture
$repoRoot = Split-Path -Parent $PSScriptRoot
$budgetBytes = 2500000

if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "crates/folia-core/assets/structure_templates.json"
}
if (-not $OutputPathMore) {
    $OutputPathMore = Join-Path $repoRoot "crates/folia-core/assets/structure_templates_more.json"
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

function Read-GzipBytes([byte[]]$Bytes) {
    $input = [System.IO.MemoryStream]::new($Bytes)
    $gzip = [System.IO.Compression.GZipStream]::new($input, [System.IO.Compression.CompressionMode]::Decompress)
    $output = [System.IO.MemoryStream]::new()
    try {
        $gzip.CopyTo($output)
        return $output.ToArray()
    } finally {
        $gzip.Dispose()
        $input.Dispose()
        $output.Dispose()
    }
}

function Convert-ToOrdinalSorted {
    param([string[]]$Values)
    $sorted = [string[]]@($Values)
    [Array]::Sort($sorted, [System.StringComparer]::Ordinal)
    return $sorted
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

$categories = Convert-ToOrdinalSorted -Values $byRoot.Keys

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

if ($outputBytes.Length -gt $budgetBytes) {
    throw ("Catalog is {0:N0} bytes; the structure-template asset budget is 2.5 MB." -f $outputBytes.Length)
}

# ---------------------------------------------------------------------------
# Second catalog: every non-village family except ExcludeRoot.
# ---------------------------------------------------------------------------

$allFiles = @(Get-ChildItem -LiteralPath $SourceRoot -Recurse -File -Filter "*.nbt")
$allPaths = [string[]]@($allFiles | ForEach-Object { $_.FullName })
[Array]::Sort($allPaths, [System.StringComparer]::Ordinal)

$excluded = [System.Collections.Generic.List[string]]::new()
if ($TemplateRoot) {
    $excluded.Add(($TemplateRoot.TrimEnd('/') + '/'))
}
if ($ExcludeRoot) {
    $excluded.Add(($ExcludeRoot.TrimEnd('/') + '/'))
}

$familyData = @{}
$familyTemplates = @{}
$moreById = @{}
$moreFamilyById = @{}
$moreCount = 0
$moreNbtBytes = 0

foreach ($path in $allPaths) {
    $relative = $path.Substring($SourceRoot.Length + 1).Replace('\', '/')
    if (-not $relative.EndsWith(".nbt", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Unexpected non-NBT file in structure tree: $relative"
    }
    $skip = $false
    foreach ($prefix in $excluded) {
        if ($relative.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            $skip = $true
            break
        }
    }
    if ($skip) {
        continue
    }

    $slash = $relative.IndexOf('/')
    if ($slash -lt 0) {
        $category = "(root)"
    } else {
        $category = $relative.Substring(0, $slash)
    }
    if (-not $familyData.ContainsKey($category)) {
        $familyData[$category] = [System.IO.MemoryStream]::new()
        $familyTemplates[$category] = [System.Collections.Generic.List[object]]::new()
    }

    $bytes = [System.IO.File]::ReadAllBytes($path)
    if ($bytes.Length -lt 18 -or $bytes[0] -ne 0x1f -or $bytes[1] -ne 0x8b) {
        throw "Template $relative is not gzip-compressed NBT."
    }
    $plain = Read-GzipBytes -Bytes $bytes
    $stream = $familyData[$category]
    $offset = $stream.Position
    $stream.Write($plain, 0, $plain.Length)

    $id = "minecraft:" + $relative.Substring(0, $relative.Length - 4)
    if ($moreById.ContainsKey($id)) {
        throw "Duplicate non-village structure template id: $id"
    }
    $record = [pscustomobject]@{
        Id = $id
        Offset = $offset
        Length = $plain.Length
    }
    $familyTemplates[$category].Add($record)
    $moreById[$id] = $record
    $moreFamilyById[$id] = $category
    $moreCount += 1
    $moreNbtBytes += $plain.Length
}

$families = Convert-ToOrdinalSorted -Values $familyData.Keys
$javaCommand = Get-Command java -ErrorAction SilentlyContinue
if (-not $javaCommand) {
    throw "Java is required to produce the level-9 gzip payloads for structure_templates_more.json."
}

$tempDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("folia-structure-more-" + [guid]::NewGuid().ToString("N"))
[void][System.IO.Directory]::CreateDirectory($tempDirectory)
try {
    $javaSourcePath = Join-Path $tempDirectory "GzipLevel9.java"
    $javaSource = @'
import java.io.*;
import java.util.zip.*;
public class GzipLevel9 {
    public static void main(String[] args) throws Exception {
        for (int index = 0; index + 1 < args.length; index += 2) {
            try (InputStream input = new BufferedInputStream(new FileInputStream(args[index]), 1 << 20);
                 GZIPOutputStream output = new GZIPOutputStream(
                         new BufferedOutputStream(new FileOutputStream(args[index + 1]), 1 << 20),
                         1 << 20) {{ def.setLevel(9); }}) {
                input.transferTo(output);
            }
        }
    }
}
'@
    [System.IO.File]::WriteAllText($javaSourcePath, $javaSource, (New-Object System.Text.UTF8Encoding($false)))

    $javaArguments = [System.Collections.Generic.List[string]]::new()
    $javaArguments.Add($javaSourcePath)
    for ($index = 0; $index -lt $families.Length; $index++) {
        $plainPath = Join-Path $tempDirectory ("{0}.nbt" -f $index)
        $gzipPath = Join-Path $tempDirectory ("{0}.gz" -f $index)
        [System.IO.File]::WriteAllBytes($plainPath, $familyData[$families[$index]].ToArray())
        $javaArguments.Add($plainPath)
        $javaArguments.Add($gzipPath)
    }

    & java @javaArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Java gzip level 9 helper failed with exit code $LASTEXITCODE."
    }

    $blobPayloads = @{}
    $blobCompressedBytes = @{}
    $totalMoreBytes = 0
    for ($index = 0; $index -lt $families.Length; $index++) {
        $family = $families[$index]
        $gzipPath = Join-Path $tempDirectory ("{0}.gz" -f $index)
        $compressed = [System.IO.File]::ReadAllBytes($gzipPath)
        $blobPayloads[$family] = [System.Convert]::ToBase64String($compressed)
        $blobCompressedBytes[$family] = $compressed.Length
        $totalMoreBytes += $compressed.Length
    }
} finally {
    Remove-Item -LiteralPath $tempDirectory -Recurse -Force -ErrorAction SilentlyContinue
}

$moreCategories = Convert-ToOrdinalSorted -Values $familyTemplates.Keys
$moreIds = Convert-ToOrdinalSorted -Values $moreById.Keys

$moreBuilder = [System.Text.StringBuilder]::new(3MB)
[void]$moreBuilder.Append('{"blobs":{')
for ($index = 0; $index -lt $families.Length; $index++) {
    if ($index -gt 0) {
        [void]$moreBuilder.Append(',')
    }
    $family = $families[$index]
    Write-JsonString -Text $family -Builder $moreBuilder
    [void]$moreBuilder.Append(':{"bytes":')
    [void]$moreBuilder.Append([System.Convert]::ToString($familyData[$family].Length, $invariant))
    [void]$moreBuilder.Append(',"payload":"')
    [void]$moreBuilder.Append($blobPayloads[$family])
    [void]$moreBuilder.Append('"}')
}
[void]$moreBuilder.Append('},"counts":{"by_root":{')
for ($index = 0; $index -lt $moreCategories.Length; $index++) {
    if ($index -gt 0) {
        [void]$moreBuilder.Append(',')
    }
    $category = $moreCategories[$index]
    Write-JsonString -Text $category -Builder $moreBuilder
    [void]$moreBuilder.Append(':')
    [void]$moreBuilder.Append([System.Convert]::ToString($familyTemplates[$category].Count, $invariant))
}
[void]$moreBuilder.Append('},"templates":')
[void]$moreBuilder.Append([System.Convert]::ToString($moreCount, $invariant))
[void]$moreBuilder.Append('},"data_version":4788,"nbt_bytes":')
[void]$moreBuilder.Append([System.Convert]::ToString($moreNbtBytes, $invariant))
[void]$moreBuilder.Append(',"templates":{')
for ($index = 0; $index -lt $moreIds.Length; $index++) {
    if ($index -gt 0) {
        [void]$moreBuilder.Append(',')
    }
    $id = $moreIds[$index]
    $record = $moreById[$id]
    Write-JsonString -Text $id -Builder $moreBuilder
    [void]$moreBuilder.Append(':{"blob":')
    Write-JsonString -Text $moreFamilyById[$id] -Builder $moreBuilder
    [void]$moreBuilder.Append(',"length":')
    [void]$moreBuilder.Append([System.Convert]::ToString($record.Length, $invariant))
    [void]$moreBuilder.Append(',"offset":')
    [void]$moreBuilder.Append([System.Convert]::ToString($record.Offset, $invariant))
    [void]$moreBuilder.Append('}')
}
[void]$moreBuilder.Append('},"total_bytes":')
[void]$moreBuilder.Append([System.Convert]::ToString($totalMoreBytes, $invariant))
[void]$moreBuilder.Append(',"version":"1.21.11"}')
[void]$moreBuilder.Append("`n")

$moreJson = $moreBuilder.ToString()
$moreOutputBytes = $utf8.GetBytes($moreJson)
$moreSha256 = Get-Sha256 $moreOutputBytes

if ($moreOutputBytes.Length -gt $budgetBytes) {
    throw ("Non-village catalog is {0:N0} bytes; the structure-template asset budget is 2.5 MB." -f $moreOutputBytes.Length)
}

if ($Check) {
    $catalogs = @(
        [pscustomobject]@{ Path = $OutputPath; Bytes = $outputBytes; Sha256 = $sha256; Count = $seen.Count },
        [pscustomobject]@{ Path = $OutputPathMore; Bytes = $moreOutputBytes; Sha256 = $moreSha256; Count = $moreCount }
    )
    foreach ($catalog in $catalogs) {
        if (-not (Test-Path -LiteralPath $catalog.Path -PathType Leaf)) {
            throw "Catalog $($catalog.Path) does not exist; run without -Check to generate it."
        }
        $existing = [System.IO.File]::ReadAllBytes($catalog.Path)
        if (-not [System.Linq.Enumerable]::SequenceEqual($existing, $catalog.Bytes)) {
            throw ("Catalog is stale: $($catalog.Path) does not match the generated bytes (generated sha256 $($catalog.Sha256)).")
        }
        Write-Host ("OK {0} ({1:N0} templates, {2:N0} bytes, sha256 {3})" -f $catalog.Path, $catalog.Count, $catalog.Bytes.Length, $catalog.Sha256)
    }
    return
}

[System.IO.File]::WriteAllBytes($OutputPath, $outputBytes)
Write-Host ("Wrote {0} ({1:N0} templates, {2:N0} bytes, {3:N0} source bytes, sha256 {4})" -f $OutputPath, $seen.Count, $outputBytes.Length, $totalBytes, $sha256)

[System.IO.File]::WriteAllBytes($OutputPathMore, $moreOutputBytes)
Write-Host ("Wrote {0} ({1:N0} templates, {2:N0} bytes, {3:N0} plain NBT bytes, sha256 {4})" -f $OutputPathMore, $moreCount, $moreOutputBytes.Length, $moreNbtBytes, $moreSha256)
