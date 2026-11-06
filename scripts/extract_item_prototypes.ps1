#requires -Version 5.1
<#
.SYNOPSIS
    Dumps the vanilla 1.21.11 item prototypes into a compact JSON catalog.

.DESCRIPTION
    Boots the local Folia/Minecraft server jar just far enough to initialize the
    built-in item registry (`Bootstrap.bootStrap`), then reads each item's
    prototype component map. No world, data pack, or network access is needed.

    The Java helper writes a tab-separated intermediate file; this script
    normalizes it into a deterministic catalog:

        crates/folia-core/assets/item_prototypes.json

    Only fields that differ from the prototype defaults are emitted, so a plain
    block item is `{}` (stack 64, no durability, common rarity). The Rust loader
    fills those defaults explicitly and rejects malformed entries.

    Output is byte-stable: items and object keys are ordinal-sorted, there is no
    insignificant whitespace, and the file ends with one LF. `-Check` regenerates
    into a temporary directory and compares the bytes instead of writing.

.PARAMETER OutputPath
    Catalog path. Defaults to crates/folia-core/assets/item_prototypes.json.

.PARAMETER ServerJar
    Folia/Minecraft server jar. Defaults to the adjacent java-worldgen tree or
    the known local copy under C:\Users\austr\Downloads\rustfol.

.PARAMETER LibraryRoot
    Directory containing the server's dependency jars. Defaults to the
    java-worldgen/libraries tree.

.PARAMETER Check
    Regenerate and compare with the existing catalog; do not write.
#>
[CmdletBinding()]
param(
    [string]$OutputPath = "",
    [string]$ServerJar = "",
    [string]$LibraryRoot = "",
    [switch]$Check
)

$ErrorActionPreference = "Stop"
$invariant = [System.Globalization.CultureInfo]::InvariantCulture
$repoRoot = Split-Path -Parent $PSScriptRoot

if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "crates/folia-core/assets/item_prototypes.json"
}

function Find-FirstFile([string[]]$Candidates) {
    foreach ($candidate in $Candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return ""
}

function Find-FirstDirectory([string[]]$Candidates) {
    foreach ($candidate in $Candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Container)) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return ""
}

if (-not $ServerJar) {
    $ServerJar = Find-FirstFile @(
        $env:FOLIA_SERVER_JAR,
        (Join-Path (Split-Path -Parent $repoRoot) "java-worldgen/versions/1.21.11/folia-1.21.11.jar"),
        (Join-Path (Split-Path -Parent $repoRoot) "folia-1.21.11-14.jar"),
        "C:\Users\austr\Downloads\rustfol\java-worldgen\versions\1.21.11\folia-1.21.11.jar",
        "C:\Users\austr\Downloads\rustfol\folia-1.21.11-14.jar"
    )
}
if (-not $ServerJar) {
    throw "Cannot find the 1.21.11 server jar; pass -ServerJar <path> or set FOLIA_SERVER_JAR."
}

if (-not $LibraryRoot) {
    $LibraryRoot = Find-FirstDirectory @(
        $env:FOLIA_SERVER_LIBRARIES,
        (Join-Path (Split-Path -Parent $repoRoot) "java-worldgen/libraries"),
        "C:\Users\austr\Downloads\rustfol\java-worldgen\libraries"
    )
}
if (-not $LibraryRoot) {
    throw "Cannot find the server dependency tree; pass -LibraryRoot <path> or set FOLIA_SERVER_LIBRARIES."
}

$javac = Get-Command javac -ErrorAction SilentlyContinue
$java = Get-Command java -ErrorAction SilentlyContinue
if (-not $javac -or -not $java) {
    throw "javac/java are required to regenerate item_prototypes.json."
}

$classPathEntries = [System.Collections.Generic.List[string]]::new()
$classPathEntries.Add($ServerJar)
foreach ($jar in @(Get-ChildItem -LiteralPath $LibraryRoot -Recurse -File -Filter "*.jar")) {
    $classPathEntries.Add($jar.FullName)
}
$classPath = [string]::Join([System.IO.Path]::PathSeparator, $classPathEntries)

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("folia-item-prototypes-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

try {
    $javaSourcePath = Join-Path $tempRoot "DumpItemPrototypes.java"
    $tsvPath = Join-Path $tempRoot "item-prototypes.tsv"
    $javaSource = @'
import com.mojang.datafixers.util.Either;
import java.io.BufferedWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Optional;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.TagKey;
import net.minecraft.world.food.FoodProperties;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.Rarity;
import net.minecraft.world.item.component.Consumable;
import net.minecraft.world.item.component.DamageResistant;
import net.minecraft.world.item.component.ItemAttributeModifiers;
import net.minecraft.world.item.component.Tool;
import net.minecraft.world.item.enchantment.Enchantable;
import net.minecraft.world.item.enchantment.Repairable;
import net.minecraft.world.item.equipment.Equippable;

/**
 * Reads only the public item/component API. The output is an intermediate TSV:
 * one line per non-air registry item, with nested values encoded as compact
 * strings. scripts/extract_item_prototypes.ps1 turns it into sorted JSON.
 */
@SuppressWarnings("unchecked")
public final class DumpItemPrototypes {
    private static final String NONE = "";
    private static final String SEP = "\t";

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        List<Item> items = new ArrayList<>();
        for (Item item : BuiltInRegistries.ITEM) {
            Identifier id = BuiltInRegistries.ITEM.getKey(item);
            if (id != null && !id.toString().equals("minecraft:air")) {
                items.add(item);
            }
        }
        items.sort(Comparator.comparing(item -> BuiltInRegistries.ITEM.getKey(item).toString()));

        Path output = Path.of(args[0]);
        try (BufferedWriter writer = Files.newBufferedWriter(output, StandardCharsets.UTF_8)) {
            writer.write("id\tmax_stack_size\tmax_damage\trarity\tenchantable\tequippable\tfood\tconsumable\tdamage_resistant\trepairable\ttool\n");
            for (Item item : items) {
                String id = BuiltInRegistries.ITEM.getKey(item).toString();
                DataComponentMap components = item.components();
                Integer maxStack = components.get(DataComponents.MAX_STACK_SIZE);
                Integer maxDamage = components.get(DataComponents.MAX_DAMAGE);
                Rarity rarity = components.getOrDefault(DataComponents.RARITY, Rarity.COMMON);
                Enchantable enchantable = components.get(DataComponents.ENCHANTABLE);
                Equippable equippable = components.get(DataComponents.EQUIPPABLE);
                FoodProperties food = components.get(DataComponents.FOOD);
                Consumable consumable = components.get(DataComponents.CONSUMABLE);
                DamageResistant damageResistant = components.get(DataComponents.DAMAGE_RESISTANT);
                Repairable repairable = components.get(DataComponents.REPAIRABLE);
                Tool tool = components.get(DataComponents.TOOL);
                ItemAttributeModifiers attributes = components.getOrDefault(
                        DataComponents.ATTRIBUTE_MODIFIERS, ItemAttributeModifiers.EMPTY);

                String[] fields = new String[] {
                    id,
                    Integer.toString(maxStack == null ? 1 : maxStack),
                    maxDamage == null ? NONE : Integer.toString(maxDamage),
                    rarity.getSerializedName(),
                    enchantable == null ? NONE : Integer.toString(enchantable.value()),
                    equippable == null ? NONE : equippable.slot().getSerializedName(),
                    food == null ? NONE : food.nutrition() + "|" + Float.toString(food.saturation()) + "|" + food.canAlwaysEat(),
                    consumable == null ? NONE : Float.toString(consumable.consumeSeconds()) + "|" + consumable.animation().getSerializedName(),
                    damageResistant == null ? NONE : "#" + damageResistant.types().location(),
                    repairable == null ? NONE : holderSet(repairable.items(), id),
                    tool == null ? NONE : toolField(tool, repairable, attributes, id)
                };
                writer.write(String.join(SEP, fields));
                writer.newLine();
            }
        }
    }

    private static String toolField(Tool tool, Repairable repairable, ItemAttributeModifiers attributes, String itemId) {
        String tier = tier(repairable, tool);
        double attackDamage = attribute(attributes, "minecraft:attack_damage");
        double attackSpeed = attribute(attributes, "minecraft:attack_speed");
        List<String> rules = new ArrayList<>();
        for (Tool.Rule rule : tool.rules()) {
            String blocks = holderSet(rule.blocks(), itemId);
            String speed = rule.speed().isPresent() ? Float.toString(rule.speed().get()) : NONE;
            String correct = rule.correctForDrops().isPresent() ? Boolean.toString(rule.correctForDrops().get()) : NONE;
            rules.add(blocks + ";" + speed + ";" + correct);
        }
        return (tier == null ? NONE : tier)
            + "|" + Float.toString(tool.defaultMiningSpeed())
            + "|" + tool.damagePerBlock()
            + "|" + tool.canDestroyBlocksInCreative()
            + "|" + (Double.isNaN(attackDamage) ? NONE : Float.toString((float) attackDamage))
            + "|" + (Double.isNaN(attackSpeed) ? NONE : Float.toString((float) attackSpeed))
            + "|" + String.join("@", rules);
    }

    private static String tier(Repairable repairable, Tool tool) {
        if (repairable != null) {
            String tag = tagOf(repairable.items());
            if (tag != null) {
                String suffix = "_tool_materials";
                if (tag.endsWith(suffix)) {
                    return normalizeTier(tag.substring(0, tag.length() - suffix.length()));
                }
            }
        }
        for (Tool.Rule rule : tool.rules()) {
            String tag = tagOf(rule.blocks());
            if (tag != null && tag.startsWith("incorrect_for_") && tag.endsWith("_tool")) {
                return normalizeTier(tag.substring("incorrect_for_".length(), tag.length() - "_tool".length()));
            }
        }
        return null;
    }

    private static String normalizeTier(String prefix) {
        int colon = prefix.indexOf(':');
        if (colon >= 0) {
            prefix = prefix.substring(colon + 1);
        }
        return switch (prefix) {
            case "wooden" -> "wood";
            case "stone", "copper", "iron", "diamond", "gold", "netherite" -> prefix;
            default -> null;
        };
    }

    private static String tagOf(HolderSet<?> set) {
        Either<?, ?> either = set.unwrap();
        if (either.left().isPresent()) {
            TagKey<?> tag = (TagKey<?>) either.left().get();
            return tag.location().toString();
        }
        return null;
    }

    private static String holderSet(HolderSet<?> set, String itemId) {
        Either<?, ?> either = set.unwrap();
        if (either.left().isPresent()) {
            TagKey<?> tag = (TagKey<?>) either.left().get();
            return "#" + tag.location();
        }
        List<String> ids = new ArrayList<>();
        for (Holder<?> holder : set) {
            Optional<ResourceKey<?>> key = (Optional<ResourceKey<?>>) (Optional<?>) holder.unwrapKey();
            if (key.isEmpty()) {
                throw new IllegalStateException("unkeyed holder in " + itemId);
            }
            ids.add(key.get().identifier().toString());
        }
        ids.sort(String::compareTo);
        return String.join(",", ids);
    }

    private static double attribute(ItemAttributeModifiers attributes, String attributeId) {
        for (ItemAttributeModifiers.Entry entry : attributes.modifiers()) {
            Optional<ResourceKey<?>> key = (Optional<ResourceKey<?>>) (Optional<?>) entry.attribute().unwrapKey();
            if (key.isPresent() && key.get().identifier().toString().equals(attributeId)) {
                return entry.modifier().amount();
            }
        }
        return Double.NaN;
    }
}
'@
    [System.IO.File]::WriteAllText($javaSourcePath, $javaSource, (New-Object System.Text.UTF8Encoding($false)))

    # Native commands write progress/warning lines to stderr. PowerShell 5.1
    # turns those into ErrorRecords, which would terminate under the script's
    # Stop preference even when the exit code is zero; capture them explicitly.
    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"

    Write-Host "Compiling item prototype oracle..."
    $compileOutput = & $javac.Source -proc:none -cp $classPath -d $tempRoot $javaSourcePath 2>&1
    $compileExit = $LASTEXITCODE
    if ($compileExit -ne 0) {
        $ErrorActionPreference = $previousErrorAction
        $compileOutput | ForEach-Object { Write-Host $_ }
        throw "javac failed with exit code $compileExit"
    }

    Write-Host "Booting item registry and dumping prototypes..."
    $runOutput = & $java.Source -cp "$tempRoot$([System.IO.Path]::PathSeparator)$classPath" DumpItemPrototypes $tsvPath 2>&1
    $runExit = $LASTEXITCODE
    $ErrorActionPreference = $previousErrorAction
    if ($runExit -ne 0) {
        $runOutput | ForEach-Object { Write-Host $_ }
        throw "DumpItemPrototypes failed with exit code $runExit"
    }

    function Get-Int([string]$Text, [string]$Context) {
        $value = 0
        if (-not [int]::TryParse($Text, [System.Globalization.NumberStyles]::Integer, $invariant, [ref]$value)) {
            throw "Invalid integer '$Text' in $Context"
        }
        return $value
    }

    function Get-Double([string]$Text, [string]$Context) {
        $value = 0.0
        if (-not [double]::TryParse($Text, [System.Globalization.NumberStyles]::Float, $invariant, [ref]$value)) {
            throw "Invalid number '$Text' in $Context"
        }
        return $value
    }

    function Get-Bool([string]$Text, [string]$Context) {
        if ($Text -eq "true") { return $true }
        if ($Text -eq "false") { return $false }
        throw "Invalid boolean '$Text' in $Context"
    }

    function Convert-IngredientSet([string]$Text, [string]$Context) {
        if ($Text.StartsWith("#")) {
            $tag = $Text.Substring(1)
            if (-not $tag.Contains(":")) {
                throw "Un-namespaced tag '$tag' in $Context"
            }
            return [ordered]@{ "tag" = $tag }
        }
        $ids = [System.Collections.Generic.List[string]]::new()
        foreach ($id in $Text.Split(",")) {
            if (-not $id) { continue }
            if (-not $id.Contains(":")) {
                throw "Un-namespaced id '$id' in $Context"
            }
            $ids.Add($id)
        }
        if ($ids.Count -eq 0) {
            throw "Empty ingredient set in $Context"
        }
        return [ordered]@{ "items" = [string[]]$ids.ToArray() }
    }

    function Convert-Tool([string]$Text, [string]$Context) {
        $parts = $Text.Split("|")
        if ($parts.Count -ne 7) {
            throw "Malformed tool '$Text' in $Context"
        }
        $tool = [ordered]@{}
        if ($parts[0]) {
            $tool["tier"] = $parts[0]
        }
        $tool["default_mining_speed"] = Get-Double $parts[1] "$Context default_mining_speed"
        $tool["damage_per_block"] = Get-Int $parts[2] "$Context damage_per_block"
        $tool["can_destroy_blocks_in_creative"] = Get-Bool $parts[3] "$Context can_destroy_blocks_in_creative"
        if ($parts[4]) {
            $tool["attack_damage"] = Get-Double $parts[4] "$Context attack_damage"
        }
        if ($parts[5]) {
            $tool["attack_speed"] = Get-Double $parts[5] "$Context attack_speed"
        }
        $rules = [System.Collections.Generic.List[object]]::new()
        if ($parts[6]) {
            foreach ($ruleText in $parts[6].Split("@")) {
                $ruleParts = $ruleText.Split(";")
                if ($ruleParts.Count -ne 3) {
                    throw "Malformed tool rule '$ruleText' in $Context"
                }
                $rule = [ordered]@{
                    "blocks" = Convert-IngredientSet $ruleParts[0] "$Context tool rule"
                }
                if ($ruleParts[1]) {
                    $rule["speed"] = Get-Double $ruleParts[1] "$Context tool rule speed"
                }
                if ($ruleParts[2]) {
                    $rule["correct_for_drops"] = Get-Bool $ruleParts[2] "$Context correct_for_drops"
                }
                $rules.Add($rule)
            }
        }
        $tool["rules"] = [object[]]$rules.ToArray()
        return $tool
    }

    $byRarity = [ordered]@{
        "common" = 0
        "uncommon" = 0
        "rare" = 0
        "epic" = 0
    }
    $counts = [ordered]@{
        "damageable" = 0
        "enchantable" = 0
        "equippable" = 0
        "fire_resistant" = 0
        "food" = 0
        "repairable" = 0
        "tool" = 0
        "total" = 0
    }
    $items = [ordered]@{}

    $lines = [System.IO.File]::ReadAllLines($tsvPath, [System.Text.Encoding]::UTF8)
    foreach ($line in $lines) {
        if (-not $line -or $line.StartsWith("id`t")) { continue }
        $fields = $line.Split([char]9)
        if ($fields.Count -ne 11) {
            throw "Malformed item row ($($fields.Count) fields): $line"
        }
        $id = $fields[0]
        if (-not $id.Contains(":")) {
            throw "Un-namespaced item id: $id"
        }
        if ($items.Contains($id)) {
            throw "Duplicate item id: $id"
        }

        $entry = [ordered]@{}
        $maxStack = Get-Int $fields[1] "$id max_stack_size"
        if ($maxStack -ne 64) {
            $entry["max_stack_size"] = $maxStack
        }
        if ($fields[2]) {
            $entry["max_damage"] = Get-Int $fields[2] "$id max_damage"
            $counts["damageable"] = $counts["damageable"] + 1
        }
        if ($fields[3] -ne "common") {
            $entry["rarity"] = $fields[3]
        }
        if (-not $byRarity.Contains($fields[3])) {
            throw "Unknown rarity '$($fields[3])' for $id"
        }
        $byRarity[$fields[3]] = $byRarity[$fields[3]] + 1

        if ($fields[4]) {
            $entry["enchantability"] = Get-Int $fields[4] "$id enchantable"
            $counts["enchantable"] = $counts["enchantable"] + 1
        }
        if ($fields[5]) {
            $entry["equippable"] = $fields[5]
            $counts["equippable"] = $counts["equippable"] + 1
        }

        $eatSeconds = 1.6
        if ($fields[7]) {
            $consumableParts = $fields[7].Split("|")
            if ($consumableParts.Count -ne 2) {
                throw "Malformed consumable '$($fields[7])' for $id"
            }
            if ($fields[6]) {
                $eatSeconds = Get-Double $consumableParts[0] "$id consume_seconds"
            }
        }
        if ($fields[6]) {
            $foodParts = $fields[6].Split("|")
            if ($foodParts.Count -ne 3) {
                throw "Malformed food '$($fields[6])' for $id"
            }
            $entry["food"] = [ordered]@{
                "can_always_eat" = Get-Bool $foodParts[2] "$id can_always_eat"
                "eat_seconds"    = $eatSeconds
                "nutrition"      = Get-Int $foodParts[0] "$id nutrition"
                "saturation"     = Get-Double $foodParts[1] "$id saturation"
            }
            $counts["food"] = $counts["food"] + 1
        }

        if ($fields[8]) {
            $damageTag = $fields[8]
            if (-not $damageTag.StartsWith("#")) {
                throw "Malformed damage_resistant '$damageTag' for $id"
            }
            if ($damageTag.Substring(1) -eq "minecraft:is_fire") {
                $entry["fire_resistant"] = $true
                $counts["fire_resistant"] = $counts["fire_resistant"] + 1
            }
        }

        if ($fields[9]) {
            $entry["repairable"] = Convert-IngredientSet $fields[9] "$id repairable"
            $counts["repairable"] = $counts["repairable"] + 1
        }

        if ($fields[10]) {
            $entry["tool"] = Convert-Tool $fields[10] $id
            $counts["tool"] = $counts["tool"] + 1
        }

        $items[$id] = $entry
        $counts["total"] = $counts["total"] + 1
    }

    $catalog = [ordered]@{}
    $catalog["counts"] = [ordered]@{
        "by_rarity"      = $byRarity
        "damageable"     = $counts["damageable"]
        "enchantable"    = $counts["enchantable"]
        "equippable"     = $counts["equippable"]
        "fire_resistant" = $counts["fire_resistant"]
        "food"           = $counts["food"]
        "repairable"     = $counts["repairable"]
        "tool"           = $counts["tool"]
        "total"          = $counts["total"]
    }
    $catalog["data_version"] = 4788
    $catalog["items"] = $items
    $catalog["version"] = "1.21.11"

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

        throw "Cannot serialize JSON value of type $($Value.GetType().FullName)"
    }

    $builder = [System.Text.StringBuilder]::new(256KB)
    Write-JsonValue -Value $catalog -Builder $builder
    $json = $builder.ToString() + "`n"
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $bytes = $utf8.GetBytes($json)

    if ($Check) {
        if (-not (Test-Path -LiteralPath $OutputPath -PathType Leaf)) {
            throw "Missing item prototype catalog: $OutputPath"
        }
        $existing = [System.IO.File]::ReadAllBytes($OutputPath)
        $different = $existing.Length -ne $bytes.Length
        if (-not $different) {
            for ($index = 0; $index -lt $bytes.Length; $index++) {
                if ($existing[$index] -ne $bytes[$index]) {
                    $different = $true
                    break
                }
            }
        }
        if ($different) {
            throw "item_prototypes.json is stale; run scripts/extract_item_prototypes.ps1"
        }
        Write-Host ("Verified {0} ({1:N0} bytes, {2:N0} items)" -f $OutputPath, $bytes.Length, $items.Count)
    } else {
        [System.IO.File]::WriteAllBytes($OutputPath, $bytes)
        Write-Host ("Wrote {0} ({1:N0} bytes, {2:N0} items)" -f $OutputPath, $bytes.Length, $items.Count)
    }
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
