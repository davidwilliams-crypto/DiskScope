<# Requires cargo-deny 0.20.2 installed from the digest-pinned package by scripts/install-cargo-deny.ps1. #>
param([switch]$Offline)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    $version = & cargo deny --version
    if ($LASTEXITCODE -ne 0 -or $version -cne 'cargo-deny 0.20.2') { throw 'Supply-chain gate requires pinned cargo-deny 0.20.2.' }
    Write-Output "Checker: $version; started UTC: $([DateTime]::UtcNow.ToString('o')); offline=$Offline"
    Write-Output "Cargo.lock SHA256: $((Get-FileHash -LiteralPath (Join-Path $projectRoot 'Cargo.lock')).Hash)"
    $metadataArgs = @('metadata', '--locked', '--all-features', '--format-version', '1', '--filter-platform', 'x86_64-pc-windows-msvc')
    if ($Offline) { $metadataArgs += '--offline' }
    # Locked metadata must resolve before checks; cargo-deny also receives --locked.
    & cargo @metadataArgs | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Locked dependency resolution failed.' }
    $denyArgs = @('deny', '--locked', '--all-features', '--target', 'x86_64-pc-windows-msvc', '--config', (Join-Path $PSScriptRoot 'helpdesk-deny.toml'))
    if ($Offline) { $denyArgs += '--offline' }
    $denyArgs += @('check', 'advisories', 'licenses', 'sources')
    & cargo @denyArgs
    $gateExitCode = $LASTEXITCODE
    $databases = Join-Path $projectRoot 'target/helpdesk-advisory-dbs'
    if (Test-Path -LiteralPath $databases) {
        foreach ($database in Get-ChildItem -LiteralPath $databases -Directory) {
            if (Test-Path -LiteralPath (Join-Path $database.FullName '.git')) {
                $revision = & git -C $database.FullName rev-parse HEAD
                if ($LASTEXITCODE -ne 0) { throw 'Could not record advisory database revision.' }
                Write-Output "Advisory database: $($database.Name); revision: $revision"
            }
        }
    }
    Write-Output "Checker exit code: $gateExitCode; finished UTC: $([DateTime]::UtcNow.ToString('o'))"
    if ($gateExitCode -ne 0) { throw 'Advisory/license/source gate failed. Fix or review the finding; do not silently add an ignore.' }
} finally { Pop-Location }
