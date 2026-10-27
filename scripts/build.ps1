param([switch]$Offline, [string]$PackageDirectory = 'dist\0.3.0', [string]$ValidationFile = 'docs\validation-360-fixes.md', [string]$PerformanceFile = 'docs\performance-c-drive-2026-10-26.md')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'package-tools.ps1')
$localCargo = Join-Path $projectRoot '.tools\cargo\bin\cargo.exe'
if (Test-Path -LiteralPath $localCargo) {
    $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
    $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
    $env:PATH = (Join-Path $env:CARGO_HOME 'bin') + ';' + $env:PATH
    $cargoCommand = $localCargo
} else { $cargoCommand = (Get-Command cargo -ErrorAction Stop).Source }
$env:CARGO_INCREMENTAL = '0'
Push-Location $projectRoot
try {
    $rustcCommand = (Get-Command rustc -ErrorAction Stop).Source
    $rustcInfo = & $rustcCommand -Vv
    if ($LASTEXITCODE -ne 0) { throw 'Compiler version inspection failed' }
    $rustRelease = ($rustcInfo | Where-Object { $_ -like 'release: *' }) -replace '^release: ', ''
    $rustCommit = ($rustcInfo | Where-Object { $_ -like 'commit-hash: *' }) -replace '^commit-hash: ', ''
    if (-not $rustRelease -or -not $rustCommit) { throw 'Compiler version inspection was incomplete' }
    $portableReadme = Join-Path $PSScriptRoot 'portable-readme.txt'
    $validationNotes = if ([IO.Path]::IsPathRooted($ValidationFile)) { $ValidationFile } else { Join-Path $projectRoot $ValidationFile }
    $performanceNotes = if ([IO.Path]::IsPathRooted($PerformanceFile)) { $PerformanceFile } else { Join-Path $projectRoot $PerformanceFile }
    $packagingNotes = Join-Path $projectRoot 'docs\packaging.md'
    foreach ($requiredFile in @($portableReadme, $validationNotes, $performanceNotes, $packagingNotes)) {
        if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) { throw "Required package file is missing: $requiredFile" }
    }
    $buildArgs = @('build','--release','--locked')
    if ($Offline) { $buildArgs += '--offline' }
    & $cargoCommand @buildArgs
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $packageRoot = if ([IO.Path]::IsPathRooted($PackageDirectory)) { $PackageDirectory } else { Join-Path $projectRoot $PackageDirectory }
    New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
    $packageRoot = (Resolve-Path -LiteralPath $packageRoot).ProviderPath
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target\release\diskscope.exe') -Destination (Join-Path $packageRoot 'DiskScope.exe') -Force
    Copy-Item -LiteralPath $portableReadme -Destination (Join-Path $packageRoot 'README.txt') -Force
    Copy-Item -LiteralPath $validationNotes -Destination (Join-Path $packageRoot 'VALIDATION.md') -Force
    Copy-Item -LiteralPath $performanceNotes -Destination (Join-Path $packageRoot 'PERFORMANCE.md') -Force
    Copy-Item -LiteralPath $packagingNotes -Destination (Join-Path $packageRoot 'packaging.md') -Force
    $metadataPath = Join-Path (Join-Path $projectRoot 'target') ('diskscope-cargo-metadata-' + [guid]::NewGuid().ToString('N') + '.json')
    $metadataArgs = @('metadata','--locked','--format-version','1','--filter-platform','x86_64-pc-windows-msvc')
    if ($Offline) { $metadataArgs += '--offline' }
    try {
        & $cargoCommand @metadataArgs | Set-Content -LiteralPath $metadataPath -Encoding utf8
        if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed' }
        & python (Join-Path $PSScriptRoot 'package-notices.py') $metadataPath $packageRoot $rustRelease $rustCommit
        if ($LASTEXITCODE -ne 0) { throw 'Notice collection failed' }
    } finally {
        if (Test-Path -LiteralPath $metadataPath) { Remove-Item -LiteralPath $metadataPath }
    }
    $hashLines = @(Get-PackageChecksums -Directory $packageRoot)
    [IO.File]::WriteAllLines((Join-Path $packageRoot 'SHA256SUMS.txt'), [string[]]$hashLines, [Text.UTF8Encoding]::new($false))
    Write-Output "Portable application: $(Join-Path $packageRoot 'DiskScope.exe')"
} finally { Pop-Location }
