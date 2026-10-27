param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CargoArgs)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$localCargo = Join-Path $projectRoot '.tools\cargo\bin\cargo.exe'
# This script runs in the caller's session, so what it sets would outlive it.
# Remember the toolchain variables first; the finally block restores them
# (a variable that was unset is removed again).
$toolchainVariables = @('RUSTUP_HOME', 'CARGO_HOME', 'PATH', 'CARGO_INCREMENTAL')
$previousEnvironment = @{}
foreach ($name in $toolchainVariables) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
# Reported after the finally block so the environment is restored on every path.
$cargoExitCode = 1
Push-Location $projectRoot
try {
    # Prefer the project-local toolchain, like build.ps1; without one, fall back
    # to the cargo on PATH.
    if (Test-Path -LiteralPath $localCargo) {
        $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
        $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
        $env:PATH = (Join-Path $env:CARGO_HOME 'bin') + ';' + $env:PATH
        $cargoCommand = $localCargo
    } else { $cargoCommand = (Get-Command cargo -ErrorAction Stop).Source }
    $env:CARGO_INCREMENTAL = '0'
    & $cargoCommand @CargoArgs
    $cargoExitCode = $LASTEXITCODE
} finally {
    Pop-Location
    foreach ($name in $toolchainVariables) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }
}
exit $cargoExitCode
