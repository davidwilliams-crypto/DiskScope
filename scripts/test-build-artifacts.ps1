param([switch]$Integration)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-tools.ps1')
. (Join-Path $PSScriptRoot 'package-tools.ps1')
$projectRoot = Split-Path -Parent $PSScriptRoot
$fixture = Join-Path $projectRoot ('target\artifact-tests\' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture -Force | Out-Null
function Assert-True([bool]$Condition, [string]$Description) {
    if (-not $Condition) { throw $Description }
}
function Assert-Rejected([scriptblock]$Operation, [string]$Description) {
    $rejected = $false
    try { & $Operation | Out-Null } catch { $rejected = $true }
    Assert-True $rejected $Description
}
try {
    $manifest = Join-Path $fixture 'Cargo.toml'
    [IO.File]::WriteAllText($manifest, "[package]`nname = `"diskscope`"`nversion = `"0.0.0`"`nedition = `"2024`"`n")
    $newExe = Join-Path $fixture 'alternate output\x86_64-pc-windows-msvc\release\diskscope.exe'
    $staleExe = Join-Path $fixture 'target\release\diskscope.exe'
    foreach ($path in @($newExe, $staleExe)) { New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null }
    [IO.File]::WriteAllText($newExe, 'new executable')
    [IO.File]::WriteAllText($staleExe, 'stale executable')
    $id = 'path+file:///fixture#diskscope@0.0.0'
    $artifact = [ordered]@{
        reason = 'compiler-artifact'; package_id = $id; manifest_path = $manifest
        target = @{ name = 'diskscope'; kind = @('bin'); crate_types = @('bin') }
        profile = @{ opt_level = '3'; test = $false }; executable = $newExe
        features = @('gui'); fresh = $false
    }
    $messages = @(($artifact | ConvertTo-Json -Depth 10 -Compress), '{"reason":"build-finished","success":true}')
    $selected = Get-ReleaseCargoArtifact -Messages $messages -PackageId $id -ManifestPath $manifest
    Assert-True ($selected.executable -ceq $newExe) 'Cargo-reported alternate output must win over a stale default executable.'
    $copied = Join-Path $fixture 'packaged.exe'
    Copy-ReleaseExecutable -Artifact $selected -Destination $copied | Out-Null
    Assert-True ([IO.File]::ReadAllText($copied) -ceq 'new executable') 'The package must contain the selected build artifact.'
    Assert-Rejected { Get-ReleaseCargoArtifact -Messages @('{"reason":"build-finished","success":true}') -PackageId $id -ManifestPath $manifest } 'Missing artifact accepted.'
    Assert-Rejected { Get-ReleaseCargoArtifact -Messages $messages -PackageId 'other-package' -ManifestPath $manifest } 'Unexpected package artifact accepted.'
    Assert-Rejected { Get-ReleaseCargoArtifact -Messages @($messages[0], $messages[0], $messages[1]) -PackageId $id -ManifestPath $manifest } 'Ambiguous artifact accepted.'
    Assert-Rejected { Get-ReleaseCargoArtifact -Messages @($messages[0], '{"reason":"build-finished","success":false}') -PackageId $id -ManifestPath $manifest } 'Failed build accepted.'
    Assert-Rejected { Get-ReleaseCargoArtifact -Messages @($messages[0]) -PackageId $id -ManifestPath $manifest } 'Unfinished build accepted.'
    $artifact.executable = Join-Path $fixture 'missing.exe'
    Assert-Rejected { Get-ReleaseCargoArtifact -Messages @(($artifact | ConvertTo-Json -Depth 10 -Compress), $messages[1]) -PackageId $id -ManifestPath $manifest } 'Missing executable accepted.'
    $metadata = [pscustomobject]@{
        packages = @(
            [pscustomobject]@{ id = 'one'; name = 'one'; version = '1'; rust_version = '1.92' },
            [pscustomobject]@{ id = 'two'; name = 'two'; version = '2'; rust_version = '1.95.0' },
            [pscustomobject]@{ id = 'inactive'; name = 'inactive'; version = '3'; rust_version = '1.99' }
        ); resolve = @{ nodes = @(@{ id = 'one' }, @{ id = 'two' }) }
    }
    Assert-True ((Get-LockedRustFloor -Metadata $metadata).minimum -ceq '1.95.0') 'The maximum active dependency floor must win; inactive packages must be ignored.'
    $metadata.packages[0].rust_version = '1.95'
    Assert-True ((Get-LockedRustFloor -Metadata $metadata).packages.Count -eq 2) 'Rust 1.95 and 1.95.0 must represent the same compiler floor.'
    $source = Get-ReleaseSourceIdentity -ProjectRoot $projectRoot
    Assert-True ($source.revision -match '^[0-9a-f]{40}$') 'Source revision must identify the checkout.'
    $cargoInput = @($source.files | Where-Object path -eq 'Cargo.toml')
    Assert-True ($cargoInput.Count -eq 1 -and $cargoInput[0].sha256 -ceq (Get-FileHash (Join-Path $projectRoot 'Cargo.toml')).Hash.ToLowerInvariant()) 'Source provenance must hash actual working-tree inputs.'
    $record = Write-ReleaseProvenance -Destination $fixture -Source $source -Artifact $selected -Build @{ target = 'x86_64-pc-windows-msvc'; arguments = @('build', '--release', '--locked') } -Executable $copied
    $provenance = Get-Content -LiteralPath $record -Raw | ConvertFrom-Json
    Assert-True ($provenance.executable.sha256 -ceq (Get-FileHash $copied).Hash.ToLowerInvariant()) 'Provenance must bind the packaged executable digest.'
    Assert-True ($provenance.source.revision -ceq $source.revision -and $provenance.build.target -ceq 'x86_64-pc-windows-msvc') 'Provenance must retain source and build configuration.'
    Assert-True (@(Get-PackageChecksums $fixture | Where-Object { $_ -like '*  BUILD_PROVENANCE.json' }).Count -eq 1) 'Package checksums must cover provenance.'
    if ($Integration) {
        New-Item -ItemType Directory -Path (Join-Path $fixture 'src') | Out-Null
        [IO.File]::WriteAllText((Join-Path $fixture 'src\main.rs'), 'fn main() { println!("fresh artifact"); }')
        $buildArgs = @('build', '--release', '--offline', '--manifest-path', $manifest, '--target', 'x86_64-pc-windows-msvc', '--message-format=json-render-diagnostics')
        $previousTargetDirectory = [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
        try {
            $env:CARGO_TARGET_DIR = Join-Path $fixture 'alternate output'
            $json = & (Join-Path $PSScriptRoot 'dev.ps1') -CargoArgs $buildArgs
            if ($LASTEXITCODE -ne 0) { throw 'Fixture Cargo build failed.' }
        } finally {
            if ($null -eq $previousTargetDirectory) { Remove-Item -LiteralPath Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
            else { [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $previousTargetDirectory, 'Process') }
        }
        $metadataJson = & (Join-Path $PSScriptRoot 'dev.ps1') -CargoArgs @('metadata', '--offline', '--no-deps', '--format-version', '1', '--manifest-path', $manifest)
        if ($LASTEXITCODE -ne 0) { throw 'Fixture metadata failed.' }
        $fixtureMetadata = $metadataJson | ConvertFrom-Json
        $actual = Get-ReleaseCargoArtifact -Messages $json -PackageId $fixtureMetadata.packages[0].id -ManifestPath $manifest
        Copy-ReleaseExecutable -Artifact $actual -Destination $copied | Out-Null
        Assert-True ($actual.executable -ceq $newExe) 'Explicit Windows target must select Cargo output under the alternate target directory.'
        Assert-True ((& $copied) -ceq 'fresh artifact') 'The packaged binary must be the freshly compiled fixture.'
        Assert-True ([IO.File]::ReadAllText($staleExe) -ceq 'stale executable') 'The stale default executable fixture must remain untouched.'
    }
    Write-Output 'Build artifact/provenance regressions passed.'
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $ownedRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'target\artifact-tests')) + '\'
    if (-not $resolved.StartsWith($ownedRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup outside artifact-tests.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
