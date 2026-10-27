param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CargoArgs)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
$env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
$env:PATH = (Join-Path $env:CARGO_HOME 'bin') + ';' + $env:PATH
$env:CARGO_INCREMENTAL = '0'
Push-Location $projectRoot
try {
    & (Join-Path $env:CARGO_HOME 'bin\cargo.exe') @CargoArgs
    exit $LASTEXITCODE
} finally { Pop-Location }
