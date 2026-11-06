#requires -Version 5.1
<#
.SYNOPSIS
    Extracts the per-state block collision and outline shapes for 1.21.11.

.DESCRIPTION
    Boots the local Folia 1.21.11 server classes, reads every state from
    `Block.BLOCK_STATE_REGISTRY`, and writes one row per state to
    `crates/folia-core/assets/block_shapes.tsv.gz`:

        id <TAB> collision <TAB> outline

    Each shape column is a semicolon-separated list of
    `minX,minY,minZ,maxX,maxY,maxZ` boxes in block-local coordinates. States
    whose block has `hasCollision == false` (air, fluids, plants, ...) get
    empty collision and outline columns, matching this port's explicit
    "passable states have no shape" policy. The table is embedded by
    `src/block_shapes.rs`; the server itself never starts a JVM for shapes.

    The dumper source is generated into a temporary directory at run time, so
    no Java source is part of the repository. A patched Folia server jar is
    required. If `-ServerJar` is not supplied, the script looks for
    `versions/1.21.11/folia-1.21.11.jar` under the reference root and, failing
    that, runs the local Paperclip jar once with `-Dpaperclip.patchonly=true`
    to materialize it (Windows junctions/copies are used for `cache` and
    `libraries`, so the reference tree stays untouched).

.PARAMETER ServerJar
    Path to the patched Folia 1.21.11 server jar. Optional when the reference
    root contains a Paperclip jar and its cache/libraries.

.PARAMETER Libraries
    Directory containing the server's dependency jars. Defaults to
    `<ReferenceRoot>/libraries` (or the sibling library tree of an already
    patched server jar).

.PARAMETER ReferenceRoot
    The local Folia reference root. Defaults to `../rustfol` next to this
    repository, then `C:\Users\austr\Downloads\rustfol`.

.PARAMETER Output
    Destination gzip file. Defaults to
    `crates/folia-core/assets/block_shapes.tsv.gz`.

.EXAMPLE
    .\scripts\extract_block_shapes.ps1 -ServerJar C:\tmp\folia\versions\1.21.11\folia-1.21.11.jar
#>
[CmdletBinding()]
param(
    [string]$ServerJar = "",
    [string]$Libraries = "",
    [string]$ReferenceRoot = "",
    [string]$Output = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Output) {
    $Output = Join-Path $repoRoot "crates/folia-core/assets/block_shapes.tsv.gz"
}
if (-not $ReferenceRoot) {
    $candidates = @(
        (Join-Path (Split-Path -Parent $repoRoot) "rustfol"),
        "C:\Users\austr\Downloads\rustfol"
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Container) {
            $ReferenceRoot = $candidate
            break
        }
    }
}
if ($ReferenceRoot) {
    $ReferenceRoot = [System.IO.Path]::GetFullPath($ReferenceRoot)
}

$expectedStates = 29671
$expectedLines = $expectedStates + 1

function Invoke-Java {
    param([string[]]$Arguments)
    $errorFile = [System.IO.Path]::GetTempFileName()
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & java @Arguments 2> $errorFile
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previous
    }
    try {
        if ($exitCode -ne 0) {
            $detail = Get-Content -LiteralPath $errorFile -Raw
            throw "java exited with code $exitCode`n$detail"
        }
    }
    finally {
        Remove-Item -LiteralPath $errorFile -Force -ErrorAction SilentlyContinue
    }
}

function Find-ServerJar {
    param([string]$Root)

    $candidates = @()
    if ($Root) {
        $candidates += (Join-Path $Root "versions/1.21.11/folia-1.21.11.jar")
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return [System.IO.Path]::GetFullPath($candidate)
        }
    }
    return ""
}

$temporaryRoots = New-Object System.Collections.Generic.List[string]

function New-TemporaryDirectory {
    param([string]$Name)
    $path = Join-Path ([System.IO.Path]::GetTempPath()) "folia-block-shapes-$Name-$([System.Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $path -Force | Out-Null
    $temporaryRoots.Add($path)
    return $path
}

function Link-Directory {
    param([string]$Link, [string]$Target)
    if (Test-Path -LiteralPath $Link) {
        return
    }
    if ($env:OS -eq "Windows_NT") {
        New-Item -ItemType Junction -Path $Link -Target $Target | Out-Null
    }
    else {
        New-Item -ItemType SymbolicLink -Path $Link -Target $Target | Out-Null
    }
}

if (-not $ServerJar) {
    $ServerJar = Find-ServerJar -Root $ReferenceRoot
}

if (-not $ServerJar) {
    if (-not $ReferenceRoot) {
        throw "No -ServerJar given and no reference root found. Pass -ServerJar <patched folia jar>."
    }
    $paperclip = Get-ChildItem -LiteralPath $ReferenceRoot -Filter "folia-*.jar" -File |
        Where-Object { $_.Name -notlike "*-sources.jar" } |
        Sort-Object Name |
        Select-Object -First 1
    if (-not $paperclip) {
        throw "No patched server jar and no Paperclip jar under $ReferenceRoot. Pass -ServerJar <patched folia jar>."
    }
    $cache = Join-Path $ReferenceRoot "cache"
    $referenceLibraries = Join-Path $ReferenceRoot "libraries"
    if (-not (Test-Path -LiteralPath $cache -PathType Container) -or
        -not (Test-Path -LiteralPath $referenceLibraries -PathType Container)) {
        throw "Paperclip needs $cache and $referenceLibraries. Pass -ServerJar <patched folia jar>."
    }

    $patchRoot = New-TemporaryDirectory -Name "patch"
    Copy-Item -LiteralPath $paperclip.FullName -Destination (Join-Path $patchRoot $paperclip.Name)
    Link-Directory -Link (Join-Path $patchRoot "cache") -Target $cache
    Link-Directory -Link (Join-Path $patchRoot "libraries") -Target $referenceLibraries
    Write-Host "Patching $($paperclip.Name) (this can take a minute) ..."
    Push-Location $patchRoot
    try {
        Invoke-Java -Arguments @("-Dpaperclip.patchonly=true", "-jar", $paperclip.Name)
    }
    finally {
        Pop-Location
    }
    $patched = Get-ChildItem -LiteralPath (Join-Path $patchRoot "versions") -Recurse -Filter "*.jar" -File |
        Sort-Object FullName |
        Select-Object -First 1
    if (-not $patched) {
        throw "Paperclip did not produce a patched server jar under $patchRoot/versions."
    }
    $ServerJar = $patched.FullName
}

$ServerJar = [System.IO.Path]::GetFullPath($ServerJar)
if (-not (Test-Path -LiteralPath $ServerJar -PathType Leaf)) {
    throw "Server jar not found: $ServerJar"
}

if (-not $Libraries) {
    $serverRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $ServerJar))
    $siblingLibraries = Join-Path $serverRoot "libraries"
    if (Test-Path -LiteralPath $siblingLibraries -PathType Container) {
        $Libraries = $siblingLibraries
    }
    elseif ($ReferenceRoot) {
        $Libraries = Join-Path $ReferenceRoot "libraries"
    }
}
if (-not $Libraries -or -not (Test-Path -LiteralPath $Libraries -PathType Container)) {
    throw "Server dependency libraries not found. Pass -Libraries <dir>."
}

$jars = Get-ChildItem -LiteralPath $Libraries -Recurse -Filter "*.jar" -File |
    Sort-Object FullName |
    ForEach-Object { $_.FullName }
if ($jars.Count -eq 0) {
    throw "No jars under $Libraries"
}
$separator = [System.IO.Path]::PathSeparator.ToString()
$classpath = $ServerJar + $separator + ($jars -join $separator)

$work = New-TemporaryDirectory -Name "dump"
$dumpSource = Join-Path $work "DumpBlockShapes.java"
$dumpTsv = Join-Path $work "block_shapes.tsv"

$source = @'
import java.io.BufferedWriter;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.CollisionContext;

public final class DumpBlockShapes {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        try (BufferedWriter out = Files.newBufferedWriter(Path.of(args[0]))) {
            out.write("id\tcollision\toutline\n");
            for (BlockState state : Block.BLOCK_STATE_REGISTRY) {
                int id = Block.BLOCK_STATE_REGISTRY.getId(state);
                Block block = state.getBlock();
                if (!block.hasCollision) {
                    out.write(id + "\t\t");
                    out.newLine();
                    continue;
                }
                List<AABB> collision = sorted(
                        state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, CollisionContext.empty())
                                .toAabbs());
                List<AABB> outline = sorted(
                        state.getShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, CollisionContext.empty())
                                .toAabbs());
                out.write(id + "\t" + format(collision) + "\t" + format(outline));
                out.newLine();
            }
        }
    }

    private static List<AABB> sorted(List<AABB> boxes) {
        List<AABB> copy = new ArrayList<>(boxes);
        copy.sort(Comparator.comparingDouble((AABB box) -> box.minX)
                .thenComparingDouble(box -> box.minY)
                .thenComparingDouble(box -> box.minZ)
                .thenComparingDouble(box -> box.maxX)
                .thenComparingDouble(box -> box.maxY)
                .thenComparingDouble(box -> box.maxZ));
        return copy;
    }

    private static String format(List<AABB> boxes) {
        StringBuilder builder = new StringBuilder();
        for (AABB box : boxes) {
            if (builder.length() > 0) {
                builder.append(';');
            }
            builder.append(box.minX).append(',')
                    .append(box.minY).append(',')
                    .append(box.minZ).append(',')
                    .append(box.maxX).append(',')
                    .append(box.maxY).append(',')
                    .append(box.maxZ);
        }
        return builder.toString();
    }
}
'@

[System.IO.File]::WriteAllText($dumpSource, $source, (New-Object System.Text.UTF8Encoding($false)))

Write-Host "Dumping block shapes from $ServerJar ..."
Invoke-Java -Arguments @("-cp", $classpath, $dumpSource, $dumpTsv)

$lines = [System.IO.File]::ReadAllLines($dumpTsv)
if ($lines.Count -ne $expectedLines) {
    throw "Expected $expectedLines shape lines, found $($lines.Count)"
}
if ($lines[0] -ne "id`tcollision`toutline") {
    throw "Unexpected shape header: $($lines[0])"
}
for ($index = 1; $index -lt $lines.Count; $index++) {
    $fields = $lines[$index].Split("`t")
    if ($fields.Count -ne 3) {
        throw "Shape row $index has $($fields.Count) fields"
    }
    $id = 0
    if (-not [int]::TryParse($fields[0], [ref]$id) -or $id -ne ($index - 1)) {
        throw "Shape row $index has non-dense id '$($fields[0])'"
    }
    foreach ($shape in @($fields[1], $fields[2])) {
        if (-not $shape) {
            continue
        }
        foreach ($box in $shape.Split(';')) {
            if ($box.Split(',').Count -ne 6) {
                throw "Shape row $index has a malformed box '$box'"
            }
        }
    }
}

$Output = [System.IO.Path]::GetFullPath($Output)
$outputDirectory = Split-Path -Parent $Output
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
    New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
}
$file = [System.IO.File]::Create($Output)
try {
    $gzip = New-Object System.IO.Compression.GZipStream(
        $file,
        [System.IO.Compression.CompressionLevel]::Optimal,
        $false)
    try {
        $writer = New-Object System.IO.StreamWriter(
            $gzip,
            (New-Object System.Text.UTF8Encoding($false)))
        try {
            $writer.NewLine = "`n"
            foreach ($line in $lines) {
                $writer.WriteLine($line)
            }
        }
        finally {
            $writer.Dispose()
        }
    }
    finally {
        $gzip.Dispose()
    }
}
finally {
    $file.Dispose()
}

$bytes = (Get-Item -LiteralPath $Output).Length
Write-Host "Wrote $Output ($expectedStates rows, $bytes bytes)"

foreach ($temporary in $temporaryRoots) {
    if (Test-Path -LiteralPath $temporary) {
        # Remove directory junctions/symlinks without following them, otherwise
        # a recursive delete could touch the reference `libraries` tree.
        Get-ChildItem -LiteralPath $temporary -Force -Directory |
            Where-Object { $_.Attributes -band [System.IO.FileAttributes]::ReparsePoint } |
            ForEach-Object { [System.IO.Directory]::Delete($_.FullName) }
        Remove-Item -LiteralPath $temporary -Recurse -Force
    }
}
