#requires -Version 5.1
<#
.SYNOPSIS
    Builds the versioned structure processor-list and template-pool catalogs.

.DESCRIPTION
    Reads the decompiled 1.21.11 data tree (../decompiled by default), normalizes
    every identifier field to a namespaced id, and emits two compact,
    deterministic JSON catalogs:

        crates/folia-core/assets/structure_processors.json
        crates/folia-core/assets/template_pools.json

    Each catalog has sorted keys, no insignificant whitespace, a trailing
    newline, and a `counts` index. Processor lists keep their direct-codec
    object shape (`{ "processors": [...] }`); template pools keep their
    `fallback`/`elements` shape. The Rust loaders embed and parse these files
    directly; the source tree is never copied verbatim.
#>
[CmdletBinding()]
param(
    [string]$SourceRoot = "",
    [string]$OutputRoot = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputRoot) {
    $OutputRoot = Join-Path $repoRoot "crates/folia-core/assets"
}
if (-not $SourceRoot) {
    $candidates = @(
        (Join-Path (Split-Path -Parent $repoRoot) "decompiled"),
        "C:\Users\austr\Downloads\rustfol\decompiled"
    )
    foreach ($candidate in $candidates) {
        if (Test-Path (Join-Path $candidate "data/minecraft/worldgen/processor_list")) {
            $SourceRoot = $candidate
            break
        }
    }
}
$worldgenRoot = Join-Path $SourceRoot "data/minecraft/worldgen"
if (-not $SourceRoot -or -not (Test-Path (Join-Path $worldgenRoot "processor_list"))) {
    throw "Cannot find decompiled/data/minecraft/worldgen/processor_list; pass -SourceRoot <decompiled>."
}
$processorRoot = Join-Path $worldgenRoot "processor_list"
$poolRoot = Join-Path $worldgenRoot "template_pool"

$invariant = [System.Globalization.CultureInfo]::InvariantCulture

# PowerShell 5.1's ConvertFrom-Json rejects case-sensitive duplicate keys, so
# use the framework serializer, which keeps ordered dictionaries.
Add-Type -AssemblyName System.Web.Extensions
$script:JsonSerializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
$script:JsonSerializer.MaxJsonLength = [int]::MaxValue

# Identifier-bearing JSON keys. Property names/values inside a block state's
# `Properties` object are not identifiers and are skipped.
$script:IdKeys = @(
    "type", "processor_type", "predicate_type", "element_type", "block",
    "tag", "location", "processors", "fallback", "feature", "loot_table",
    "Name", "value", "final_state"
)

function Normalize-IdString([string]$Text) {
    if ([string]::IsNullOrEmpty($Text)) {
        return $Text
    }
    $prefix = ""
    $value = $Text
    if ($value.StartsWith("#")) {
        $prefix = "#"
        $value = $value.Substring(1)
    }
    if ($value.Contains(":")) {
        return "$prefix$value"
    }
    if ($value -match '^[a-z0-9_.-]+(/[a-z0-9_.-]+)*$') {
        return "$prefix" + "minecraft:" + $value
    }
    return $Text
}

function Normalize-BlockStateString([string]$Text) {
    if ([string]::IsNullOrEmpty($Text)) {
        return $Text
    }
    $bracket = $Text.IndexOf("[")
    if ($bracket -lt 0) {
        return Normalize-IdString $Text
    }
    $name = $Text.Substring(0, $bracket)
    $rest = $Text.Substring($bracket)
    return (Normalize-IdString $name) + $rest
}

function Normalize-InPlace {
    param($Value, [string]$ParentKey = "")

    if ($null -eq $Value -or $Value -is [string]) {
        return
    }

    if ($ParentKey -eq "Properties" -or $ParentKey -eq "properties") {
        return
    }

    if ($Value -is [System.Collections.IList]) {
        for ($index = 0; $index -lt $Value.Count; $index++) {
            $item = $Value[$index]
            if ($item -is [string]) {
                if ($script:IdKeys -contains $ParentKey) {
                    if ($ParentKey -eq "final_state") {
                        $Value[$index] = Normalize-BlockStateString $item
                    } else {
                        $Value[$index] = Normalize-IdString $item
                    }
                }
            } else {
                Normalize-InPlace -Value $item -ParentKey $ParentKey
            }
        }
        return
    }

    if ($Value -is [System.Collections.IDictionary]) {
        foreach ($childKey in @($Value.Keys)) {
            $child = $Value[$childKey]
            if ($child -is [string]) {
                if ($script:IdKeys -contains $childKey) {
                    if ($childKey -eq "final_state") {
                        $Value[$childKey] = Normalize-BlockStateString $child
                    } else {
                        $Value[$childKey] = Normalize-IdString $child
                    }
                }
            } else {
                Normalize-InPlace -Value $child -ParentKey $childKey
            }
        }
    }
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

function Write-JsonValue {
    param($Value, [System.Text.StringBuilder]$Builder)

    if ($null -eq $Value) {
        [void]$Builder.Append("null")
        return
    }
    if ($Value -is [bool]) {
        [void]$Builder.Append($(if ($Value) { "true" } else { "false" }))
        return
    }
    if ($Value -is [string]) {
        Write-JsonString -Text $Value -Builder $Builder
        return
    }
    if ($Value -is [int] -or $Value -is [long] -or $Value -is [int16] -or
        $Value -is [byte] -or $Value -is [sbyte] -or $Value -is [uint32] -or
        $Value -is [uint64] -or $Value -is [uint16]) {
        [void]$Builder.Append([System.Convert]::ToString($Value, $invariant))
        return
    }
    if ($Value -is [double] -or $Value -is [single] -or $Value -is [decimal]) {
        [void]$Builder.Append(([double]$Value).ToString("R", $invariant))
        return
    }

    if ($Value -is [System.Collections.IDictionary]) {
        $keys = [System.Collections.Generic.List[string]]::new()
        foreach ($key in $Value.Keys) {
            $keys.Add([string]$key)
        }
        $keys.Sort([System.StringComparer]::Ordinal)
        [void]$Builder.Append('{')
        $first = $true
        foreach ($key in $keys) {
            if (-not $first) {
                [void]$Builder.Append(',')
            }
            $first = $false
            Write-JsonString -Text $key -Builder $Builder
            [void]$Builder.Append(':')
            Write-JsonValue -Value $Value[$key] -Builder $Builder
        }
        [void]$Builder.Append('}')
        return
    }

    if ($Value -is [System.Collections.IList]) {
        [void]$Builder.Append('[')
        for ($index = 0; $index -lt $Value.Count; $index++) {
            if ($index -gt 0) {
                [void]$Builder.Append(',')
            }
            Write-JsonValue -Value $Value[$index] -Builder $Builder
        }
        [void]$Builder.Append(']')
        return
    }

    if ($Value.PSObject) {
        $keys = [System.Collections.Generic.List[string]]::new()
        foreach ($property in $Value.PSObject.Properties) {
            $keys.Add($property.Name)
        }
        $keys.Sort([System.StringComparer]::Ordinal)
        [void]$Builder.Append('{')
        $first = $true
        foreach ($key in $keys) {
            if (-not $first) {
                [void]$Builder.Append(',')
            }
            $first = $false
            Write-JsonString -Text $key -Builder $Builder
            [void]$Builder.Append(':')
            Write-JsonValue -Value $Value.$key -Builder $Builder
        }
        [void]$Builder.Append('}')
        return
    }

    throw "Cannot serialize JSON value of type $($Value.GetType().FullName)"
}

function ConvertTo-CompactJson($Value) {
    $builder = [System.Text.StringBuilder]::new(1MB)
    Write-JsonValue -Value $Value -Builder $builder
    return $builder.ToString()
}

function Get-SortedJsonPaths([string]$Directory) {
    $files = @(Get-ChildItem -Path $Directory -Recurse -File -Filter "*.json")
    $paths = [string[]]@($files | ForEach-Object { $_.FullName })
    [Array]::Sort($paths, [System.StringComparer]::Ordinal)
    return $paths
}

function Get-ResourceId([string]$Directory, [string]$Path) {
    $relative = $Path.Substring($Directory.Length + 1).Replace('\', '/')
    return "minecraft:" + $relative.Substring(0, $relative.Length - 5)
}

function Add-Count {
    param($Counts, [string]$Key, [int]$Amount = 1)
    if (-not $Counts.ContainsKey($Key)) {
        $Counts[$Key] = 0
    }
    $Counts[$Key] = $Counts[$Key] + $Amount
}

function Build-ProcessorCatalog {
    $entries = [ordered]@{}
    $byType = @{}
    $processorCount = 0

    foreach ($path in Get-SortedJsonPaths $processorRoot) {
        $id = Get-ResourceId $processorRoot $path
        if ($entries.Contains($id)) {
            throw "Duplicate processor-list id: $id"
        }

        $raw = [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
        $data = $script:JsonSerializer.DeserializeObject($raw)
        Normalize-InPlace -Value $data
        if (-not ($data -is [System.Collections.IDictionary]) -or -not $data.ContainsKey("processors")) {
            throw "Processor list $id is missing the processors field"
        }
        $processors = $data["processors"]
        if (-not ($processors -is [System.Collections.IList])) {
            throw "Processor list $id has a non-list processors field"
        }
        foreach ($processor in $processors) {
            if (-not ($processor -is [System.Collections.IDictionary]) -or -not $processor.ContainsKey("processor_type")) {
                throw "Processor list $id has a processor without processor_type"
            }
            $type = [string]$processor["processor_type"]
            Add-Count -Counts $byType -Key $type
            $processorCount++
        }
        $entries[$id] = $data
    }

    $typeKeys = [string[]]@($byType.Keys)
    [Array]::Sort($typeKeys, [System.StringComparer]::Ordinal)
    $orderedTypes = [ordered]@{}
    foreach ($type in $typeKeys) {
        $orderedTypes[$type] = $byType[$type]
    }

    $catalog = [ordered]@{}
    $catalog["counts"] = [ordered]@{
        "by_type"    = $orderedTypes
        "processors" = $processorCount
        "total"      = $entries.Count
    }
    $catalog["data_version"] = 4788
    $catalog["processor_lists"] = $entries
    $catalog["version"] = "1.21.11"
    return $catalog
}

function Count-PoolElement {
    param($Element)

    if (-not ($Element -is [System.Collections.IDictionary])) {
        throw "Template-pool element must be an object"
    }
    if (-not $Element.ContainsKey("element_type")) {
        throw "Template-pool element is missing element_type"
    }

    $script:elementNodes++
    Add-Count -Counts $script:elementTypes -Key ([string]$Element["element_type"])
    if ($Element.ContainsKey("projection") -and $null -ne $Element["projection"]) {
        Add-Count -Counts $script:elementProjections -Key ([string]$Element["projection"])
    }
    if ($Element.ContainsKey("elements")) {
        $children = $Element["elements"]
        if (-not ($children -is [System.Collections.IList])) {
            throw "List pool element has a non-list elements field"
        }
        foreach ($child in $children) {
            Count-PoolElement -Element $child
        }
    }
}

function Build-PoolCatalog {
    $entries = [ordered]@{}
    $elementTypes = @{}
    $elementProjections = @{}
    $script:elementTypes = $elementTypes
    $script:elementProjections = $elementProjections
    $script:elementNodes = 0
    $entryCount = 0
    $templateCount = 0

    foreach ($path in Get-SortedJsonPaths $poolRoot) {
        $id = Get-ResourceId $poolRoot $path
        if ($entries.Contains($id)) {
            throw "Duplicate template-pool id: $id"
        }

        $raw = [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
        $data = $script:JsonSerializer.DeserializeObject($raw)
        Normalize-InPlace -Value $data
        if (-not ($data -is [System.Collections.IDictionary])) {
            throw "Template pool $id must be an object"
        }
        if (-not $data.ContainsKey("fallback")) {
            throw "Template pool $id is missing the fallback field"
        }
        if (-not $data.ContainsKey("elements") -or -not ($data["elements"] -is [System.Collections.IList])) {
            throw "Template pool $id is missing the elements list"
        }

        foreach ($entry in $data["elements"]) {
            if (-not ($entry -is [System.Collections.IDictionary]) -or -not $entry.ContainsKey("element") -or -not $entry.ContainsKey("weight")) {
                throw "Template pool $id has an entry without element/weight"
            }
            $weight = [int]$entry["weight"]
            if ($weight -lt 1 -or $weight -gt 150) {
                throw "Template pool $id has weight $weight outside 1..=150"
            }
            $entryCount++
            $templateCount += $weight
            Count-PoolElement -Element $entry["element"]
        }

        $entries[$id] = $data
    }

    $typeKeys = [string[]]@($elementTypes.Keys)
    [Array]::Sort($typeKeys, [System.StringComparer]::Ordinal)
    $orderedTypes = [ordered]@{}
    foreach ($type in $typeKeys) {
        $orderedTypes[$type] = $elementTypes[$type]
    }

    $projectionKeys = [string[]]@($elementProjections.Keys)
    [Array]::Sort($projectionKeys, [System.StringComparer]::Ordinal)
    $orderedProjections = [ordered]@{}
    foreach ($projection in $projectionKeys) {
        $orderedProjections[$projection] = $elementProjections[$projection]
    }

    $catalog = [ordered]@{}
    $catalog["counts"] = [ordered]@{
        "by_element_type" = $orderedTypes
        "by_projection"   = $orderedProjections
        "elements"        = $script:elementNodes
        "entries"         = $entryCount
        "pools"           = $entries.Count
        "templates"       = $templateCount
    }
    $catalog["data_version"] = 4788
    $catalog["template_pools"] = $entries
    $catalog["version"] = "1.21.11"
    return $catalog
}

function Write-Catalog {
    param($Catalog, [string]$Path)

    $json = ConvertTo-CompactJson $Catalog
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $json + "`n", $utf8)
    $bytes = (Get-Item $Path).Length
    Write-Host ("Wrote {0} ({1:N0} bytes)" -f $Path, $bytes)
}

Write-Host "Source: $SourceRoot"
$processorCatalog = Build-ProcessorCatalog
Write-Catalog -Catalog $processorCatalog -Path (Join-Path $OutputRoot "structure_processors.json")

$poolCatalog = Build-PoolCatalog
Write-Catalog -Catalog $poolCatalog -Path (Join-Path $OutputRoot "template_pools.json")
