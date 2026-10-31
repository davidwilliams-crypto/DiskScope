<#
Installs the exact Rust toolchain CI validates with, then proves the installed
compiler is the upstream release it claims to be.

rustup verifies every downloaded component against the checksums in the
distribution channel manifest, but that manifest is fetched from the network
at install time. The release and commit hash recorded below therefore fail the
build before any validation output is trusted if the distribution ever serves
compiler bits other than upstream rustc 1.92.0
(ded5c06cf21d2b93bffd5d884aa6e96934ee4234, 2025-12-08). Update the pinned
identity here together with the floor in Cargo.toml when it moves;
scripts/check-rust-floor.ps1 keeps that floor honest.
#>
param(
    [string]$Version = '1.92.0',
    [string]$CommitHash = 'ded5c06cf21d2b93bffd5d884aa6e96934ee4234',
    [string[]]$Component = @(),
    [string]$Target = 'x86_64-pc-windows-msvc'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$installArgs = @('toolchain', 'install', $Version, '--profile', 'minimal', '--target', $Target)
foreach ($extra in $Component) { $installArgs += @('--component', $extra) }
& rustup @installArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& rustup override set $Version
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$details = & rustc -vV
if ($LASTEXITCODE -ne 0) { throw 'rustc version inspection failed.' }
$release = ($details | Where-Object { $_ -like 'release: *' }) -replace '^release: ', ''
$commit = ($details | Where-Object { $_ -like 'commit-hash: *' }) -replace '^commit-hash: ', ''
if ($release -cne $Version -or $commit -cne $CommitHash) {
    throw "Installed toolchain is rustc $release ($commit); expected $Version ($CommitHash). Refusing to validate with an unverified compiler."
}
$cargoVersion = & cargo --version
if ($LASTEXITCODE -ne 0 -or $cargoVersion -notlike "cargo $Version (*") {
    throw "Installed cargo is '$cargoVersion'; expected the cargo shipped with rustc $Version."
}
Write-Output "Pinned toolchain verified: rustc $release ($commit); $cargoVersion."
