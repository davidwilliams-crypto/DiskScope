$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-tools.ps1')
$json = & (Join-Path $PSScriptRoot 'dev.ps1') -CargoArgs @('metadata', '--locked', '--format-version', '1', '--filter-platform', 'x86_64-pc-windows-msvc')
if ($LASTEXITCODE -ne 0) { throw 'Locked Windows dependency metadata failed.' }
$metadata = $json | ConvertFrom-Json
$floor = Get-LockedRustFloor -Metadata $metadata
$root = $metadata.packages | Where-Object id -eq $metadata.resolve.root
if (-not $root -or (ConvertTo-RustReleaseVersion $root.rust_version) -lt (ConvertTo-RustReleaseVersion $floor.minimum)) { throw "Declared minimum must cover locked Windows dependency floor $($floor.minimum)." }
Write-Output "Locked Windows dependency floor: $($floor.minimum) ($($floor.packages -join ', ')); declared: $($root.rust_version)"
