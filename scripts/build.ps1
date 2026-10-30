param([switch]$Offline, [string]$PackageDirectory, [string]$ValidationFile = 'docs\validation-360-fixes.md', [string]$PerformanceFile = 'docs\performance-c-drive-2026-10-26.md')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'package-tools.ps1')
. (Join-Path $PSScriptRoot 'release-tools.ps1')
# The package directory follows Cargo.toml so a version bump cannot leave the
# build in a stale dist folder that the benchmark and release scripts never read.
$version = Get-ReleaseVersion -ProjectRoot $projectRoot
if (-not $PackageDirectory) { $PackageDirectory = "dist\$version" }
# The package README is a copy of this file. Assert-ReleasePayload would reject a
# stale version only after the full release build, so check it before compiling.
$portableReadme = Join-Path $PSScriptRoot 'portable-readme.txt'
Assert-ReleaseReadmeVersion -Path $portableReadme -Version $version
$localCargo = Join-Path $projectRoot '.tools\cargo\bin\cargo.exe'
# This script runs in the caller's session, so what it sets would outlive it.
# Remember the toolchain variables first; the final finally block restores them
# (a variable that was unset is removed again).
$toolchainVariables = @('RUSTUP_HOME', 'CARGO_HOME', 'PATH', 'CARGO_INCREMENTAL')
$previousEnvironment = @{}
foreach ($name in $toolchainVariables) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$previousOutputEncoding = [Console]::OutputEncoding
$metadataPath = $null
Push-Location $projectRoot
try {
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    if (Test-Path -LiteralPath $localCargo) {
        $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
        $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
        $env:PATH = (Join-Path $env:CARGO_HOME 'bin') + ';' + $env:PATH
        $cargoCommand = $localCargo
    } else { $cargoCommand = (Get-Command cargo -ErrorAction Stop).Source }
    $env:CARGO_INCREMENTAL = '0'
    $rustcCommand = (Get-Command rustc -ErrorAction Stop).Source
    $rustcInfo = & $rustcCommand -Vv
    if ($LASTEXITCODE -ne 0) { throw 'Compiler version inspection failed' }
    $rustRelease = ($rustcInfo | Where-Object { $_ -like 'release: *' }) -replace '^release: ', ''
    $rustCommit = ($rustcInfo | Where-Object { $_ -like 'commit-hash: *' }) -replace '^commit-hash: ', ''
    if (-not $rustRelease -or -not $rustCommit) { throw 'Compiler version inspection was incomplete' }
    $validationNotes = if ([IO.Path]::IsPathRooted($ValidationFile)) { $ValidationFile } else { Join-Path $projectRoot $ValidationFile }
    $performanceNotes = if ([IO.Path]::IsPathRooted($PerformanceFile)) { $PerformanceFile } else { Join-Path $projectRoot $PerformanceFile }
    $packagingNotes = Join-Path $projectRoot 'docs\packaging.md'
    foreach ($requiredFile in @($portableReadme, $validationNotes, $performanceNotes, $packagingNotes)) {
        if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) { throw "Required package file is missing: $requiredFile" }
    }
    # Packages are Windows x64. An explicit target overrides build.target while
    # Cargo's JSON artifact path still honors target-dir/build-dir configuration.
    $target = 'x86_64-pc-windows-msvc'
    $metadataArgs = @('metadata','--locked','--format-version','1','--filter-platform',$target)
    if ($Offline) { $metadataArgs += '--offline' }
    $metadataJson = & $cargoCommand @metadataArgs
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed' }
    $metadata = $metadataJson | ConvertFrom-Json
    $manifest = Join-Path $projectRoot 'Cargo.toml'
    $rootPackage = @($metadata.packages | Where-Object { [IO.Path]::GetFullPath($_.manifest_path) -ieq $manifest })
    if ($rootPackage.Count -ne 1) { throw 'Expected one root Cargo package.' }
    $floor = Get-LockedRustFloor -Metadata $metadata
    if ((ConvertTo-RustReleaseVersion $rootPackage[0].rust_version) -lt (ConvertTo-RustReleaseVersion $floor.minimum)) { throw "Declared Rust minimum is below the locked Windows dependency floor $($floor.minimum)." }
    $source = Get-ReleaseSourceIdentity -ProjectRoot $projectRoot
    $configurationFiles = @{}
    $candidate = $projectRoot
    while ($candidate) {
        foreach ($name in @('config', 'config.toml')) {
            $configPath = Join-Path $candidate ".cargo\$name"
            if (Test-Path -LiteralPath $configPath -PathType Leaf) { $configurationFiles[$configPath] = (Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash.ToLowerInvariant() }
        }
        $candidate = [IO.Path]::GetDirectoryName($candidate)
    }
    $cargoHomePath = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    foreach ($name in @('config', 'config.toml')) {
        $configPath = Join-Path $cargoHomePath $name
        if (Test-Path -LiteralPath $configPath -PathType Leaf) { $configurationFiles[$configPath] = (Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $buildEnvironment = @{}
    foreach ($entry in Get-ChildItem Env:) {
        if ($entry.Name -match '^(CARGO_(BUILD_|TARGET_|PROFILE_|ENCODED_RUSTFLAGS$|HOME$)|RUST(FLAGS$|C$|C_WRAPPER$|C_WORKSPACE_WRAPPER$|UP_TOOLCHAIN$))') { $buildEnvironment[$entry.Name] = $entry.Value }
    }
    $cargoInfo = & $cargoCommand -Vv
    if ($LASTEXITCODE -ne 0) { throw 'Cargo version inspection failed' }
    $buildArgs = @('build','--release','--locked','--bin','diskscope','--target',$target,'--message-format=json-render-diagnostics')
    if ($Offline) { $buildArgs += '--offline' }
    $messages = & $cargoCommand @buildArgs
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $artifact = Get-ReleaseCargoArtifact -Messages $messages -PackageId $rootPackage[0].id -ManifestPath $manifest
    if ((Get-ReleaseSourceIdentity -ProjectRoot $projectRoot).fingerprint -cne $source.fingerprint) { throw 'Source files changed during the release build; retry from a stable checkout.' }
    $packageRoot = if ([IO.Path]::IsPathRooted($PackageDirectory)) { $PackageDirectory } else { Join-Path $projectRoot $PackageDirectory }
    Assert-NoReleaseReparsePath $packageRoot
    New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
    $packageRoot = (Resolve-Path -LiteralPath $packageRoot).ProviderPath
    Assert-NoReleaseReparsePath $packageRoot
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
    $packagedExecutable = Join-Path $packageRoot 'DiskScope.exe'
    Copy-ReleaseExecutable -Artifact $artifact -Destination $packagedExecutable | Out-Null
    Copy-Item -LiteralPath $portableReadme -Destination (Join-Path $packageRoot 'README.txt') -Force
    Copy-Item -LiteralPath $validationNotes -Destination (Join-Path $packageRoot 'VALIDATION.md') -Force
    Copy-Item -LiteralPath $performanceNotes -Destination (Join-Path $packageRoot 'PERFORMANCE.md') -Force
    Copy-Item -LiteralPath $packagingNotes -Destination (Join-Path $packageRoot 'packaging.md') -Force
    $metadataPath = [IO.Path]::GetTempFileName()
    [IO.File]::WriteAllText($metadataPath, ($metadataJson -join "`n"), [Text.UTF8Encoding]::new($false))
    & python (Join-Path $PSScriptRoot 'package-notices.py') $metadataPath $packageRoot $rustRelease $rustCommit
    if ($LASTEXITCODE -ne 0) { throw 'Notice collection failed' }
    if ((Get-ReleaseSourceIdentity -ProjectRoot $projectRoot).fingerprint -cne $source.fingerprint) { throw 'Source files changed during packaging; retry from a stable checkout.' }
    $buildConfiguration = [ordered]@{
        target = $target; arguments = $buildArgs; rustc = @($rustcInfo); cargo = @($cargoInfo)
        environment = $buildEnvironment; cargo_config_sha256 = $configurationFiles; locked_rust_floor = $floor
        cargo_lock_sha256 = (Get-FileHash -LiteralPath (Join-Path $projectRoot 'Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    Write-ReleaseProvenance -Destination $packageRoot -Source $source -Artifact $artifact -Build $buildConfiguration -Executable $packagedExecutable | Out-Null
    $hashLines = @(Get-PackageChecksums -Directory $packageRoot)
    [IO.File]::WriteAllLines((Join-Path $packageRoot 'SHA256SUMS.txt'), [string[]]$hashLines, [Text.UTF8Encoding]::new($false))
    Write-Output "Portable application: $(Join-Path $packageRoot 'DiskScope.exe')"
} finally {
    [Console]::OutputEncoding = $previousOutputEncoding
    if ($metadataPath -and (Test-Path -LiteralPath $metadataPath)) { Remove-Item -LiteralPath $metadataPath }
    Pop-Location
    foreach ($name in $toolchainVariables) {
        if ($null -eq $previousEnvironment[$name]) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process') }
    }
}
