param([switch]$Offline, [string]$PackageDirectory, [string]$ValidationFile = 'docs\validation-360-fixes.md', [string]$PerformanceFile = 'docs\performance-c-drive-2026-10-26.md')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'package-tools.ps1')
. (Join-Path $PSScriptRoot 'release-tools.ps1')
# The package directory follows Cargo.toml so a version bump cannot leave the
# build in a stale dist folder that the benchmark and release scripts never read.
$version = Get-ReleaseVersion -ProjectRoot $projectRoot
if (-not $PackageDirectory) { $PackageDirectory = "dist\$version" }
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
    # Remove generated output from an earlier build of this version. Nothing else
    # is touched, so a user-supplied directory never loses unrelated files; but a
    # leftover licenses\<old-crate> folder must not be hashed into the manifest.
    foreach ($generated in @('licenses', 'THIRD_PARTY_NOTICES.md', 'SHA256SUMS.txt')) {
        $stale = Join-Path $packageRoot $generated
        if (Test-Path -LiteralPath $stale) {
            if ((Get-Item -LiteralPath $stale -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing to clean a reparse point: $stale" }
            Remove-Item -LiteralPath $stale -Recurse -Force
        }
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target\release\diskscope.exe') -Destination (Join-Path $packageRoot 'DiskScope.exe') -Force
    Copy-Item -LiteralPath $portableReadme -Destination (Join-Path $packageRoot 'README.txt') -Force
    Copy-Item -LiteralPath $validationNotes -Destination (Join-Path $packageRoot 'VALIDATION.md') -Force
    Copy-Item -LiteralPath $performanceNotes -Destination (Join-Path $packageRoot 'PERFORMANCE.md') -Force
    Copy-Item -LiteralPath $packagingNotes -Destination (Join-Path $packageRoot 'packaging.md') -Force
    $metadataPath = Join-Path (Join-Path $projectRoot 'target') ('diskscope-cargo-metadata-' + [guid]::NewGuid().ToString('N') + '.json')
    $metadataArgs = @('metadata','--locked','--format-version','1','--filter-platform','x86_64-pc-windows-msvc')
    if ($Offline) { $metadataArgs += '--offline' }
    # cargo prints UTF-8 JSON; PowerShell decodes native output with the console
    # code page, which would corrupt non-ASCII manifest paths (e.g. a user profile
    # with accented characters) before they are written back out.
    $previousOutputEncoding = [Console]::OutputEncoding
    try {
        [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
        & $cargoCommand @metadataArgs | Set-Content -LiteralPath $metadataPath -Encoding utf8
        if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed' }
        [Console]::OutputEncoding = $previousOutputEncoding
        & python (Join-Path $PSScriptRoot 'package-notices.py') $metadataPath $packageRoot $rustRelease $rustCommit
        if ($LASTEXITCODE -ne 0) { throw 'Notice collection failed' }
    } finally {
        [Console]::OutputEncoding = $previousOutputEncoding
        if (Test-Path -LiteralPath $metadataPath) { Remove-Item -LiteralPath $metadataPath }
    }
    $hashLines = @(Get-PackageChecksums -Directory $packageRoot)
    [IO.File]::WriteAllLines((Join-Path $packageRoot 'SHA256SUMS.txt'), [string[]]$hashLines, [Text.UTF8Encoding]::new($false))
    Write-Output "Portable application: $(Join-Path $packageRoot 'DiskScope.exe')"
} finally { Pop-Location }
