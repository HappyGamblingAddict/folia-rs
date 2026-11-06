#requires -Version 5.1
<#
.SYNOPSIS
    Builds the versioned loot-table and recipe catalogs.

.DESCRIPTION
    Reads the decompiled 1.21.11 data tree (../decompiled by default), normalizes
    every identifier field to a namespaced id, and emits two compact,
    deterministic JSON catalogs:

        crates/folia-core/assets/loot_tables.json
        crates/folia-core/assets/recipes.json

    Each catalog has a sorted top-level object, sorted object keys, no
    insignificant whitespace, a trailing newline, and a `counts` index with the
    total and per-type entry counts. The Rust loaders in src/loot.rs embed and
    parse these files directly; the source tree is never copied verbatim.
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
        if (Test-Path (Join-Path $candidate "data/minecraft/loot_table")) {
            $SourceRoot = $candidate
            break
        }
    }
}
if (-not $SourceRoot -or -not (Test-Path (Join-Path $SourceRoot "data/minecraft/loot_table"))) {
    throw "Cannot find decompiled/data/minecraft/loot_table; pass -SourceRoot <decompiled>."
}

$lootRoot = Join-Path $SourceRoot "data/minecraft/loot_table"
$recipeRoot = Join-Path $SourceRoot "data/minecraft/recipe"

$invariant = [System.Globalization.CultureInfo]::InvariantCulture

# PowerShell 5.1's ConvertFrom-Json rejects case-sensitive duplicate keys
# (`anvil.json` has both "I" and "i"), so use the framework serializer, which
# keeps a case-sensitive StringComparer and ordered dictionaries.
Add-Type -AssemblyName System.Web.Extensions
$script:JsonSerializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
$script:JsonSerializer.MaxJsonLength = [int]::MaxValue

# Identifier-bearing JSON keys. Only these fields are namespaced, so recipe
# categories ("misc", "equipment") and state-property values ("lower") are not
# mistaken for ids.
$script:IdKeys = @(
    "type", "name", "tag", "loot_table", "enchantment", "potion", "id",
    "random_sequence", "function", "condition", "formula", "value",
    "enchantments", "material", "input", "ingredient", "ingredients",
    "template", "base", "addition", "items"
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

function Normalize-IngredientValue($Value) {
    if ($Value -is [string]) {
        return Normalize-IdString $Value
    }
    if ($Value -is [System.Collections.IList]) {
        for ($index = 0; $index -lt $Value.Count; $index++) {
            $Value[$index] = Normalize-IngredientValue $Value[$index]
        }
        return $Value
    }
    return $Value
}

function Normalize-InPlace {
    param($Value, [string]$ParentKey = "")

    if ($null -eq $Value -or $Value -is [string]) {
        return
    }

    # Block-state property values are compared as strings ("double", "lower",
    # "3"); a property may be named "type" without being an id.
    if ($ParentKey -eq "properties") {
        return
    }

    if ($Value -is [System.Collections.IList]) {
        for ($index = 0; $index -lt $Value.Count; $index++) {
            $item = $Value[$index]
            if ($item -is [string]) {
                if ($script:IdKeys -contains $ParentKey -or $ParentKey -eq "key") {
                    $Value[$index] = Normalize-IdString $item
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
            if ($ParentKey -eq "key") {
                $Value[$childKey] = Normalize-IngredientValue $child
            } elseif ($child -is [string]) {
                if ($script:IdKeys -contains $childKey) {
                    $Value[$childKey] = Normalize-IdString $child
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

function Build-Catalog {
    param(
        [string]$Directory,
        [string]$RootKey,
        [string]$DefaultType
    )

    $files = @(Get-ChildItem -Path $Directory -Recurse -File -Filter "*.json")
    $paths = [string[]]@($files | ForEach-Object { $_.FullName })
    [Array]::Sort($paths, [System.StringComparer]::Ordinal)

    $entries = [ordered]@{}
    $byType = @{}
    foreach ($path in $paths) {
        $relative = $path.Substring($Directory.Length + 1).Replace('\', '/')
        $id = "minecraft:" + $relative.Substring(0, $relative.Length - 5)
        if ($entries.Contains($id)) {
            throw "Duplicate $RootKey id: $id"
        }

        $raw = [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
        $data = $script:JsonSerializer.DeserializeObject($raw)
        Normalize-InPlace -Value $data

        $type = $DefaultType
        if ($data -is [System.Collections.IDictionary] -and $data.ContainsKey("type")) {
            $type = [string]$data["type"]
        }
        if (-not $byType.ContainsKey($type)) {
            $byType[$type] = 0
        }
        $byType[$type] = $byType[$type] + 1
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
        "by_type" = $orderedTypes
        "total"   = $entries.Count
    }
    $catalog["data_version"] = 4788
    $catalog[$RootKey] = $entries
    $catalog["version"] = "1.21.11"
    return $catalog
}

function Write-Catalog {
    param($Catalog, [string]$Path)

    $json = ConvertTo-CompactJson $Catalog
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $json + "`n", $utf8)
    $bytes = (Get-Item $Path).Length
    Write-Host ("Wrote {0} ({1:N0} bytes, {2:N0} entries)" -f $Path, $bytes, $Catalog["counts"]["total"])
}

Write-Host "Source: $SourceRoot"
$lootCatalog = Build-Catalog -Directory $lootRoot -RootKey "tables" -DefaultType "minecraft:generic"
Write-Catalog -Catalog $lootCatalog -Path (Join-Path $OutputRoot "loot_tables.json")

$recipeCatalog = Build-Catalog -Directory $recipeRoot -RootKey "recipes" -DefaultType "minecraft:crafting_special_unknown"
Write-Catalog -Catalog $recipeCatalog -Path (Join-Path $OutputRoot "recipes.json")
