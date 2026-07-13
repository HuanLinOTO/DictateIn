param(
    [string]$OutputDirectory = "dist",
    [string]$ModelDirectory = "target\debug\models",
    [switch]$SkipBuild,
    [switch]$SkipArchive
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$outputPath = Join-Path $projectRoot $OutputDirectory
$bundlePath = Join-Path $outputPath "DictateIn"
$archivePath = Join-Path $outputPath "DictateIn-windows-x64.zip"

Push-Location $projectRoot
try {
    if (-not $SkipBuild) {
        cargo build --release --locked
    }

    if (-not (Test-Path -LiteralPath $outputPath)) {
        New-Item -ItemType Directory -Path $outputPath | Out-Null
    }
    if (Test-Path -LiteralPath $bundlePath) {
        Remove-Item -LiteralPath $bundlePath -Recurse -Force
    }
    New-Item -ItemType Directory -Path $bundlePath | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $bundlePath "config") | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $bundlePath "models") | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $bundlePath "logs") | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $bundlePath "cache") | Out-Null

    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\dictate-in.exe") -Destination $bundlePath

    $resolvedModelDirectory = Join-Path $projectRoot $ModelDirectory
    if (-not (Test-Path -LiteralPath $resolvedModelDirectory)) {
        throw "Model directory does not exist: $resolvedModelDirectory"
    }
    Get-ChildItem -LiteralPath $resolvedModelDirectory -Directory | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $bundlePath "models") -Recurse
    }

    Get-ChildItem -LiteralPath (Join-Path $projectRoot "target\release") -Filter "*.dll" | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $bundlePath
    }

    if ($SkipArchive) {
        Write-Output $bundlePath
        return
    }

    if (Test-Path -LiteralPath $archivePath) {
        Remove-Item -LiteralPath $archivePath -Force
    }
    Compress-Archive -LiteralPath $bundlePath -DestinationPath $archivePath
    Write-Output $archivePath
}
finally {
    Pop-Location
}
