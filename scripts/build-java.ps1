param(
    [string]$LibrariesDirectory = $env:FOLIA_LIBRARIES,
    [string]$ServerJar = $env:FOLIA_SERVER_JAR,
    [switch]$SkipJavaTests
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$jdk = if ($env:JAVA_HOME) { $env:JAVA_HOME } else { "C:\Program Files\Java\jdk-21" }
$javac = Join-Path $jdk "bin\javac.exe"
$java = Join-Path $jdk "bin\java.exe"
$jarTool = Join-Path $jdk "bin\jar.exe"

if (!(Test-Path $javac)) {
    throw "javac not found at $javac. Set JAVA_HOME to a JDK 21 installation."
}

function Find-FirstDirectory([string[]]$Candidates) {
    foreach ($candidate in $Candidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return (Resolve-Path $candidate).Path
        }
    }
    return $null
}

if (!$LibrariesDirectory) {
    $LibrariesDirectory = Find-FirstDirectory @(
        (Join-Path $root "libraries"),
        (Join-Path (Split-Path -Parent $root) "libraries"),
        "C:\Users\austr\Downloads\rustfol\libraries"
    )
}
if (!$LibrariesDirectory -or !(Test-Path $LibrariesDirectory)) {
    throw "No dependency jars found. Pass -LibrariesDirectory <dir> or set FOLIA_LIBRARIES."
}

if (!$ServerJar) {
    $ServerJar = Find-FirstDirectory @(
        (Join-Path $root "versions\1.21.11"),
        (Join-Path $root "..\versions\1.21.11"),
        "C:\Users\austr\Downloads\rustfol\versions\1.21.11"
    )
}
if ($ServerJar -and (Test-Path (Join-Path $ServerJar "folia-1.21.11.jar"))) {
    $ServerJar = Join-Path $ServerJar "folia-1.21.11.jar"
} elseif ($ServerJar -and !(Test-Path $ServerJar)) {
    $ServerJar = $null
}

$separator = [IO.Path]::PathSeparator
$libraryJars = Get-ChildItem $LibrariesDirectory -Recurse -Filter *.jar |
    ForEach-Object { $_.FullName }
if ($libraryJars.Count -eq 0) {
    throw "No dependency jars found under $LibrariesDirectory. Reconstruct the server first."
}
$classpath = $libraryJars -join $separator
if ($ServerJar) {
    $classpath = $classpath + $separator + $ServerJar
}

$bridgeSource = Join-Path $root "java\bridge\src"
$bridgeClasses = Join-Path $root "java\bridge\classes"
$bridgeJar = Join-Path $root "java\bridge\bridge.jar"
Remove-Item -Recurse -Force $bridgeClasses -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $bridgeClasses | Out-Null

function Invoke-Native([string]$Tool, [string[]]$Arguments) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $Tool @Arguments 2>&1 | ForEach-Object { Write-Host $_ }
    $code = $LASTEXITCODE
    $ErrorActionPreference = $previous
    return $code
}

$bridgeFiles = Get-ChildItem $bridgeSource -Recurse -Filter *.java |
    ForEach-Object { $_.FullName }
$bridgeArguments = @("-proc:none", "-encoding", "UTF-8", "-source", "21",
    "-target", "21", "-classpath", $classpath, "-d", $bridgeClasses) + $bridgeFiles
if ((Invoke-Native $javac $bridgeArguments) -ne 0) {
    throw "Bridge compilation failed."
}
Copy-Item -Recurse -Force (Join-Path $root "java\bridge\resources\*") $bridgeClasses
if ((Invoke-Native $jarTool @("--create", "--file", $bridgeJar, "-C", $bridgeClasses, ".")) -ne 0) {
    throw "Bridge packaging failed."
}

$pluginSource = Join-Path $root "java\example-plugin\src"
$pluginClasses = Join-Path $root "java\example-plugin\classes"
$pluginJar = Join-Path $root "plugins\HelloPlugin.jar"
Remove-Item -Recurse -Force $pluginClasses -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $pluginClasses | Out-Null

$pluginFiles = Get-ChildItem $pluginSource -Recurse -Filter *.java |
    ForEach-Object { $_.FullName }
$pluginClasspath = $classpath + $separator + $bridgeClasses
$pluginArguments = @("-proc:none", "-encoding", "UTF-8", "-source", "21",
    "-target", "21", "-classpath", $pluginClasspath, "-d", $pluginClasses) + $pluginFiles
if ((Invoke-Native $javac $pluginArguments) -ne 0) {
    throw "Example plugin compilation failed."
}
Copy-Item (Join-Path $pluginSource "plugin.yml") (Join-Path $pluginClasses "plugin.yml") -Force
Copy-Item (Join-Path $pluginSource "config.yml") (Join-Path $pluginClasses "config.yml") -Force
New-Item -ItemType Directory -Force (Split-Path -Parent $pluginJar) | Out-Null
if ((Invoke-Native $jarTool @("--create", "--file", $pluginJar, "-C", $pluginClasses, ".")) -ne 0) {
    throw "Example plugin packaging failed."
}

$bridgeTestSource = Join-Path $root "java\bridge\test"
if ((Test-Path $bridgeTestSource) -and !$SkipJavaTests) {
    $bridgeTestClasses = Join-Path $root "java\bridge\test-classes"
    Remove-Item -Recurse -Force $bridgeTestClasses -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $bridgeTestClasses | Out-Null
    $bridgeTestFiles = Get-ChildItem $bridgeTestSource -Recurse -Filter *.java |
        ForEach-Object { $_.FullName }
    $testArguments = @("-proc:none", "-encoding", "UTF-8", "-source", "21",
        "-target", "21", "-classpath", ($classpath + $separator + $bridgeClasses),
        "-d", $bridgeTestClasses) + $bridgeTestFiles
    if ((Invoke-Native $javac $testArguments) -ne 0) {
        throw "Bridge test compilation failed."
    }
    $testClasspath = $bridgeTestClasses + $separator + $bridgeClasses + $separator + $classpath
    if ((Invoke-Native $java @("-cp", $testClasspath, "rs.folia.bridge.BridgeApiTest")) -ne 0) {
        throw "Bridge API test failed."
    }
}

Write-Host "Built $bridgeJar"
Write-Host "Built $pluginJar"
