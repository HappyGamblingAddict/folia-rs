#requires -Version 5.1
<#
.SYNOPSIS
    Extracts the vanilla 1.21.11 tag registries into flat TSV assets.

.DESCRIPTION
    Reads the decompiled reference tree (read-only) and writes one compact
    `tag<TAB>member` table per registry under
    `crates/folia-core/assets/tags/`. Nested `#tag` references are resolved
    recursively, de-duplicated, and sorted by tag then member. Empty tags emit
    no line.

    The output is deterministic: files, tag keys and members are all sorted
    with ordinal comparison and written as UTF-8 without a BOM.

.PARAMETER ReferenceTags
    Root of the decompiled `data/minecraft/tags` tree. Defaults to
    `../decompiled/data/minecraft/tags` relative to the repository root.

.PARAMETER OutputDirectory
    Destination directory. Defaults to `crates/folia-core/assets/tags`.
#>
[CmdletBinding()]
param(
    [string]$ReferenceTags = "",
    [string]$OutputDirectory = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($ReferenceTags)) {
    $ReferenceTags = Join-Path $repoRoot "..\decompiled\data\minecraft\tags"
}
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repoRoot "crates\folia-core\assets\tags"
}
$ReferenceTags = [System.IO.Path]::GetFullPath($ReferenceTags)
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)

if (-not (Test-Path -LiteralPath $ReferenceTags -PathType Container)) {
    throw "Reference tag tree not found: $ReferenceTags"
}

# Registry directory name -> output asset name. The worldgen directory holds
# four subregistries (biome, structure, flat_level_generator_preset,
# world_preset); their tag keys do not collide in 1.21.11, so they share one
# asset and the directory prefix is stripped from the key.
$registries = @(
    @{ Name = "block"; Asset = "block.tsv" },
    @{ Name = "item"; Asset = "item.tsv" },
    @{ Name = "entity_type"; Asset = "entity_type.tsv" },
    @{ Name = "fluid"; Asset = "fluid.tsv" },
    @{ Name = "game_event"; Asset = "game_event.tsv" },
    @{ Name = "damage_type"; Asset = "damage_type.tsv" },
    @{ Name = "enchantment"; Asset = "enchantment.tsv" },
    @{ Name = "point_of_interest_type"; Asset = "point_of_interest_type.tsv" },
    @{ Name = "worldgen"; Asset = "worldgen.tsv" }
)

function Add-Namespace([string]$Key) {
    if ($Key.Contains(":")) {
        return $Key
    }
    return "minecraft:$Key"
}

# A worldgen file's path is `<subregistry>/<tag path>.json`; the tag id is the
# path after the subregistry directory. Other registries use their whole
# relative path.
function Get-TagKey([string]$Registry, [string]$RelativePath) {
    $key = $RelativePath -replace '\.json$', ''
    $key = $key -replace '\\', '/'
    if ($Registry -eq "worldgen") {
        $separator = $key.IndexOf('/')
        if ($separator -lt 0) {
            throw "Worldgen tag has no subregistry directory: $RelativePath"
        }
        $key = $key.Substring($separator + 1)
    }
    return Add-Namespace $key
}

function Read-TagFile([string]$Path) {
    $json = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    $values = @()
    if ($null -ne $json.values) {
        $values = @($json.values)
    }

    $entries = New-Object System.Collections.Generic.List[string]
    foreach ($value in $values) {
        $id = $null
        if ($value -is [string]) {
            $id = $value
        } elseif ($null -ne $value.id) {
            # Datapack-style {id, required}; vanilla ships only strings.
            $id = [string]$value.id
        }
        if ([string]::IsNullOrWhiteSpace($id)) {
            continue
        }
        $entries.Add($id.Trim())
    }
    return ,$entries.ToArray()
}

$tagFiles = @{}
foreach ($registry in $registries) {
    $directory = Join-Path $ReferenceTags $registry.Name
    if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
        throw "Missing tag registry directory: $directory"
    }

    $relativePaths = @(
        Get-ChildItem -LiteralPath $directory -Recurse -File -Filter *.json |
            ForEach-Object { $_.FullName.Substring($directory.Length + 1) }
    )
    [Array]::Sort($relativePaths, [System.StringComparer]::Ordinal)

    $tags = @{}
    foreach ($relativePath in $relativePaths) {
        $tagKey = Get-TagKey $registry.Name $relativePath
        if ($tags.ContainsKey($tagKey)) {
            throw "Duplicate tag key '$tagKey' in registry '$($registry.Name)'"
        }
        $path = Join-Path $directory $relativePath
        # `replace` only matters when merging datapack overlays; this tree is
        # the complete vanilla set, so each file's values are authoritative.
        $tags[$tagKey] = Read-TagFile $path
    }
    $tagFiles[$registry.Name] = $tags
}

# Resolve nested references per registry with memoization. A back-edge is a
# data error (vanilla's loader rejects cycles), so fail loudly instead of
# emitting a partial member list.
$memo = @{}
$resolving = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)

function Resolve-Tag([string]$Registry, [string]$TagKey) {
    $registryMemo = $memo[$Registry]
    if ($null -eq $registryMemo) {
        $registryMemo = @{}
        $memo[$Registry] = $registryMemo
    }
    if ($registryMemo.ContainsKey($TagKey)) {
        return ,$registryMemo[$TagKey]
    }

    $cycleKey = "$Registry|$TagKey"
    if (-not $resolving.Add($cycleKey)) {
        throw "Cyclic tag reference in registry '$Registry': $TagKey"
    }

    $members = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)
    foreach ($entry in $tagFiles[$Registry][$TagKey]) {
        if ($entry.StartsWith('#')) {
            $target = Add-Namespace $entry.Substring(1)
            if (-not $tagFiles[$Registry].ContainsKey($target)) {
                throw "Unresolved tag reference '$entry' in '$TagKey' (registry '$Registry')"
            }
            $nested = Resolve-Tag $Registry $target
            foreach ($member in $nested) {
                [void]$members.Add($member)
            }
        } else {
            [void]$members.Add((Add-Namespace $entry))
        }
    }

    [void]$resolving.Remove($cycleKey)
    $sorted = New-Object System.Collections.Generic.List[string]
    foreach ($member in $members) {
        $sorted.Add($member)
    }
    $sorted.Sort([System.StringComparer]::Ordinal)
    $registryMemo[$TagKey] = $sorted.ToArray()
    return ,$registryMemo[$TagKey]
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)

$totalTags = 0
$totalMembers = 0
$summary = New-Object System.Collections.Generic.List[string]

foreach ($registry in $registries) {
    $tags = $tagFiles[$registry.Name]
    $tagKeys = New-Object System.Collections.Generic.List[string]
    foreach ($key in $tags.Keys) {
        $tagKeys.Add($key)
    }
    $tagKeys.Sort([System.StringComparer]::Ordinal)

    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($tagKey in $tagKeys) {
        $members = Resolve-Tag $registry.Name $tagKey
        foreach ($member in $members) {
            $lines.Add("$tagKey`t$member")
        }
    }

    $text = [string]::Join("`n", $lines)
    $path = Join-Path $OutputDirectory $registry.Asset
    [System.IO.File]::WriteAllText($path, $text, $utf8)

    $bytes = [System.Text.Encoding]::UTF8.GetByteCount($text)
    $emptyTags = @($tagKeys | Where-Object { $tags[$_].Count -eq 0 }).Count
    $summary.Add(("{0}: {1} tags ({2} empty), {3} members, {4} bytes -> {5}" -f `
                $registry.Name, $tagKeys.Count, $emptyTags, $lines.Count, $bytes, $registry.Asset))
    $totalTags += $tagKeys.Count
    $totalMembers += $lines.Count
}

Write-Host "Extracted vanilla tags from $ReferenceTags"
foreach ($line in $summary) {
    Write-Host "  $line"
}
Write-Host "Total: $totalTags tags, $totalMembers flattened members"
Write-Host "Output: $OutputDirectory"
