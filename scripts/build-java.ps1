$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$jdk = if ($env:JAVA_HOME) { $env:JAVA_HOME } else { "C:\Program Files\Java\jdk-21" }
$javac = Join-Path $jdk "bin\javac.exe"
$jarTool = Join-Path $jdk "bin\jar.exe"

if (!(Test-Path $javac)) {
    throw "javac not found at $javac. Set JAVA_HOME to a JDK 21 installation."
}

$separator = [IO.Path]::PathSeparator
$librariesDir = Join-Path $root "libraries"
if (!(Test-Path $librariesDir)) {
    $librariesDir = Join-Path (Split-Path -Parent $root) "libraries"
}
if (!(Test-Path $librariesDir)) {
    throw "No dependency jars found under $root\libraries or its parent. Reconstruct the server first."
}
$libraries = Get-ChildItem $librariesDir -Recurse -Filter *.jar |
    ForEach-Object { $_.FullName }
if ($libraries.Count -eq 0) {
    throw "No dependency jars found under $librariesDir. Reconstruct the server first."
}
$classpath = $libraries -join $separator

$bridgeSource = Join-Path $root "java\bridge\src"
$bridgeClasses = Join-Path $root "java\bridge\classes"
$bridgeJar = Join-Path $root "java\bridge\bridge.jar"
Remove-Item -Recurse -Force $bridgeClasses -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $bridgeClasses | Out-Null

$bridgeFiles = Get-ChildItem $bridgeSource -Recurse -Filter *.java |
    ForEach-Object { $_.FullName }
& $javac -encoding UTF-8 -source 21 -target 21 -classpath $classpath -d $bridgeClasses $bridgeFiles
if ($LASTEXITCODE -ne 0) {
    throw "Bridge compilation failed."
}
Copy-Item -Recurse -Force (Join-Path $root "java\bridge\resources\*") $bridgeClasses
& $jarTool --create --file $bridgeJar -C $bridgeClasses .
if ($LASTEXITCODE -ne 0) {
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
& $javac -encoding UTF-8 -source 21 -target 21 -classpath $pluginClasspath -d $pluginClasses $pluginFiles
if ($LASTEXITCODE -ne 0) {
    throw "Example plugin compilation failed."
}
Copy-Item (Join-Path $pluginSource "plugin.yml") (Join-Path $pluginClasses "plugin.yml") -Force
New-Item -ItemType Directory -Force (Split-Path -Parent $pluginJar) | Out-Null
& $jarTool --create --file $pluginJar -C $pluginClasses .
if ($LASTEXITCODE -ne 0) {
    throw "Example plugin packaging failed."
}

Write-Host "Built $bridgeJar"
Write-Host "Built $pluginJar"
