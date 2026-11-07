#requires -Version 5.1
<#
.SYNOPSIS
    Builds the embedded vanilla structure-template catalogs.

.DESCRIPTION
    Reads the decompiled 1.21.11 structure NBT tree (../decompiled by default)
    and writes three deterministic JSON catalogs:

        crates/folia-core/assets/structure_templates.json
        crates/folia-core/assets/structure_templates_more.json
        crates/folia-core/assets/structure_templates_trial.json

    The first catalog contains every template under `village/`. Its entries are
    keyed by normalized structure id and each value is the base64 encoding of
    the template's original gzip-compressed NBT bytes.

    The second catalog contains the remaining non-village template families.
    `trial_chambers` is excluded because it ships in the third catalog. A
    719-entry per-template catalog would exceed the 2.5 MB budget, so each
    catalog is stored as one base64+gzip NBT stream per family (or trial part)
    plus an offset/length index:

        "blobs":     { "<family>": { "bytes": <plain NBT bytes>, "payload": "..." } }
        "templates": { "<id>": { "blob": "<family>", "length": N, "offset": N } }

    The third catalog contains every template under `trial_chambers/`, split by
    its first subdirectory (`chamber`, `chests`, `corridor`, ...). Splitting the
    191 templates into parts keeps each compressed stream compact while only
    adding a few hundred bytes versus one solid stream. Every slice is a
    complete gzip-decoded NBT document.

    Ids and object keys are ordinal-sorted, there is no insignificant
    whitespace, and each file ends with one LF, so repeated runs are
    byte-identical. The indexed catalogs are recompressed with Java's zlib
    `Deflater` level 9; that dependency is required only for the second and
    third files.

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

.PARAMETER OutputPathTrial
    Trial-chamber catalog path. Defaults to
    crates/folia-core/assets/structure_templates_trial.json.

.PARAMETER TemplateRoot
    Subdirectory of the structure tree for the first catalog. Defaults to
    `village`.

.PARAMETER TrialRoot
    Subdirectory of the structure tree for the third catalog. Defaults to
    `trial_chambers`.

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
    [string]$OutputPathTrial = "",
    [string]$TemplateRoot = "village",
    [string]$TrialRoot = "trial_chambers",
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
if (-not $OutputPathTrial) {
    $OutputPathTrial = Join-Path $repoRoot "crates/folia-core/assets/structure_templates_trial.json"
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

# ---------------------------------------------------------------------------
# Third catalog: trial_chambers, split into one stream per subdirectory.
# ---------------------------------------------------------------------------

$trialRootPath = Join-Path $SourceRoot $TrialRoot
if (-not (Test-Path -LiteralPath $trialRootPath -PathType Container)) {
    throw "Trial template root '$TrialRoot' does not exist under $SourceRoot."
}

$trialFiles = @(Get-ChildItem -LiteralPath $trialRootPath -Recurse -File -Filter "*.nbt")
$trialPaths = [string[]]@($trialFiles | ForEach-Object { $_.FullName })
[Array]::Sort($trialPaths, [System.StringComparer]::Ordinal)

$trialPartData = @{}
$trialPartTemplates = @{}
$trialById = @{}
$trialPartById = @{}
$trialCount = 0
$trialNbtBytes = 0

foreach ($path in $trialPaths) {
    $relative = $path.Substring($SourceRoot.Length + 1).Replace('\', '/')
    if (-not $relative.EndsWith(".nbt", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Unexpected non-NBT file in trial structure tree: $relative"
    }
    $withinTrial = $path.Substring($trialRootPath.Length + 1).Replace('\', '/')
    $slash = $withinTrial.IndexOf('/')
    if ($slash -lt 0) {
        $part = "(root)"
    } else {
        $part = $withinTrial.Substring(0, $slash)
    }
    if (-not $trialPartData.ContainsKey($part)) {
        $trialPartData[$part] = [System.IO.MemoryStream]::new()
        $trialPartTemplates[$part] = [System.Collections.Generic.List[object]]::new()
    }

    $bytes = [System.IO.File]::ReadAllBytes($path)
    if ($bytes.Length -lt 18 -or $bytes[0] -ne 0x1f -or $bytes[1] -ne 0x8b) {
        throw "Trial template $relative is not gzip-compressed NBT."
    }
    $plain = Read-GzipBytes -Bytes $bytes
    $stream = $trialPartData[$part]
    $offset = $stream.Position
    $stream.Write($plain, 0, $plain.Length)

    $id = "minecraft:" + $relative.Substring(0, $relative.Length - 4)
    if ($trialById.ContainsKey($id)) {
        throw "Duplicate trial-chamber structure template id: $id"
    }
    $record = [pscustomobject]@{
        Id = $id
        Offset = $offset
        Length = $plain.Length
    }
    $trialPartTemplates[$part].Add($record)
    $trialById[$id] = $record
    $trialPartById[$id] = $part
    $trialCount += 1
    $trialNbtBytes += $plain.Length
}

if ($trialCount -eq 0) {
    throw "No trial-chamber templates found under $trialRootPath."
}

$families = Convert-ToOrdinalSorted -Values $familyData.Keys
$trialParts = Convert-ToOrdinalSorted -Values $trialPartData.Keys
$javaCommand = Get-Command java -ErrorAction SilentlyContinue
if (-not $javaCommand) {
    throw "Java is required to produce the level-9 gzip payloads for the indexed structure-template catalogs."
}

$tempDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("folia-structure-indexed-" + [guid]::NewGuid().ToString("N"))
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
        $plainPath = Join-Path $tempDirectory ("more-{0}.nbt" -f $index)
        $gzipPath = Join-Path $tempDirectory ("more-{0}.gz" -f $index)
        [System.IO.File]::WriteAllBytes($plainPath, $familyData[$families[$index]].ToArray())
        $javaArguments.Add($plainPath)
        $javaArguments.Add($gzipPath)
    }
    for ($index = 0; $index -lt $trialParts.Length; $index++) {
        $plainPath = Join-Path $tempDirectory ("trial-{0}.nbt" -f $index)
        $gzipPath = Join-Path $tempDirectory ("trial-{0}.gz" -f $index)
        [System.IO.File]::WriteAllBytes($plainPath, $trialPartData[$trialParts[$index]].ToArray())
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
        $gzipPath = Join-Path $tempDirectory ("more-{0}.gz" -f $index)
        $compressed = [System.IO.File]::ReadAllBytes($gzipPath)
        $blobPayloads[$family] = [System.Convert]::ToBase64String($compressed)
        $blobCompressedBytes[$family] = $compressed.Length
        $totalMoreBytes += $compressed.Length
    }

    $trialBlobPayloads = @{}
    $trialBlobCompressedBytes = @{}
    $totalTrialBytes = 0
    for ($index = 0; $index -lt $trialParts.Length; $index++) {
        $part = $trialParts[$index]
        $gzipPath = Join-Path $tempDirectory ("trial-{0}.gz" -f $index)
        $compressed = [System.IO.File]::ReadAllBytes($gzipPath)
        $trialBlobPayloads[$part] = [System.Convert]::ToBase64String($compressed)
        $trialBlobCompressedBytes[$part] = $compressed.Length
        $totalTrialBytes += $compressed.Length
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

$trialCategories = Convert-ToOrdinalSorted -Values $trialPartTemplates.Keys
$trialIds = Convert-ToOrdinalSorted -Values $trialById.Keys

$trialBuilder = [System.Text.StringBuilder]::new(2MB)
[void]$trialBuilder.Append('{"blobs":{')
for ($index = 0; $index -lt $trialParts.Length; $index++) {
    if ($index -gt 0) {
        [void]$trialBuilder.Append(',')
    }
    $part = $trialParts[$index]
    Write-JsonString -Text $part -Builder $trialBuilder
    [void]$trialBuilder.Append(':{"bytes":')
    [void]$trialBuilder.Append([System.Convert]::ToString($trialPartData[$part].Length, $invariant))
    [void]$trialBuilder.Append(',"payload":"')
    [void]$trialBuilder.Append($trialBlobPayloads[$part])
    [void]$trialBuilder.Append('"}')
}
[void]$trialBuilder.Append('},"counts":{"by_root":{')
for ($index = 0; $index -lt $trialCategories.Length; $index++) {
    if ($index -gt 0) {
        [void]$trialBuilder.Append(',')
    }
    $category = $trialCategories[$index]
    Write-JsonString -Text $category -Builder $trialBuilder
    [void]$trialBuilder.Append(':')
    [void]$trialBuilder.Append([System.Convert]::ToString($trialPartTemplates[$category].Count, $invariant))
}
[void]$trialBuilder.Append('},"templates":')
[void]$trialBuilder.Append([System.Convert]::ToString($trialCount, $invariant))
[void]$trialBuilder.Append('},"data_version":4788,"nbt_bytes":')
[void]$trialBuilder.Append([System.Convert]::ToString($trialNbtBytes, $invariant))
[void]$trialBuilder.Append(',"templates":{')
for ($index = 0; $index -lt $trialIds.Length; $index++) {
    if ($index -gt 0) {
        [void]$trialBuilder.Append(',')
    }
    $id = $trialIds[$index]
    $record = $trialById[$id]
    Write-JsonString -Text $id -Builder $trialBuilder
    [void]$trialBuilder.Append(':{"blob":')
    Write-JsonString -Text $trialPartById[$id] -Builder $trialBuilder
    [void]$trialBuilder.Append(',"length":')
    [void]$trialBuilder.Append([System.Convert]::ToString($record.Length, $invariant))
    [void]$trialBuilder.Append(',"offset":')
    [void]$trialBuilder.Append([System.Convert]::ToString($record.Offset, $invariant))
    [void]$trialBuilder.Append('}')
}
[void]$trialBuilder.Append('},"total_bytes":')
[void]$trialBuilder.Append([System.Convert]::ToString($totalTrialBytes, $invariant))
[void]$trialBuilder.Append(',"version":"1.21.11"}')
[void]$trialBuilder.Append("`n")

$trialJson = $trialBuilder.ToString()
$trialOutputBytes = $utf8.GetBytes($trialJson)
$trialSha256 = Get-Sha256 $trialOutputBytes

if ($trialOutputBytes.Length -gt $budgetBytes) {
    throw ("Trial-chamber catalog is {0:N0} bytes; the structure-template asset budget is 2.5 MB." -f $trialOutputBytes.Length)
}

if ($Check) {
    $catalogs = @(
        [pscustomobject]@{ Path = $OutputPath; Bytes = $outputBytes; Sha256 = $sha256; Count = $seen.Count },
        [pscustomobject]@{ Path = $OutputPathMore; Bytes = $moreOutputBytes; Sha256 = $moreSha256; Count = $moreCount },
        [pscustomobject]@{ Path = $OutputPathTrial; Bytes = $trialOutputBytes; Sha256 = $trialSha256; Count = $trialCount }
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

[System.IO.File]::WriteAllBytes($OutputPathTrial, $trialOutputBytes)
Write-Host ("Wrote {0} ({1:N0} templates, {2:N0} bytes, {3:N0} plain NBT bytes, sha256 {4})" -f $OutputPathTrial, $trialCount, $trialOutputBytes.Length, $trialNbtBytes, $trialSha256)
