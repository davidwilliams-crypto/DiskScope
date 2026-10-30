$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-upgrade-tools.ps1')
$script:checks = 0
function Assert-Rejected([scriptblock]$Operation, [string]$Message) {
    $rejected = $false
    try { & $Operation | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw $Message }; $script:checks++
}
$fixtureEnvironment = @{
    GITHUB_ACTIONS = 'true'; RUNNER_ENVIRONMENT = 'github-hosted'; RUNNER_OS = 'Windows'
    GITHUB_SERVER_URL = 'https://github.com'; GITHUB_REPOSITORY = 'davidwilliams-crypto/DiskScope'
    GITHUB_EVENT_NAME = 'push'; GITHUB_SHA = ('a' * 40); GITHUB_RUN_ID = '123'
}
Assert-InstallerUpgradeHostedEnvironment $fixtureEnvironment
$script:checks++
foreach ($key in @($fixtureEnvironment.Keys)) {
    $missing = $fixtureEnvironment.Clone(); $missing.Remove($key)
    Assert-Rejected { Assert-InstallerUpgradeHostedEnvironment $missing } "Missing hosted guard accepted: $key"
}
$selfHosted = $fixtureEnvironment.Clone(); $selfHosted.RUNNER_ENVIRONMENT = 'self-hosted'
Assert-Rejected { Assert-InstallerUpgradeHostedEnvironment $selfHosted } 'Self-hosted machine accepted.'
Assert-InstallerUpgradeCleanState @() @() @()
$script:checks++
Assert-Rejected { Assert-InstallerUpgradeCleanState @('existing-registration') @() @() } 'Existing registration accepted.'
Assert-Rejected { Assert-InstallerUpgradeCleanState @() @('existing-shortcut-or-folder') @() } 'Existing path accepted.'
Assert-Rejected { Assert-InstallerUpgradeCleanState @() @() @('running-app') } 'Running app accepted.'
Assert-Rejected { Assert-InstallerUpgradeOwnedLocation 'C:\fixture\install' $null } 'Unregistered uninstaller accepted.'
Assert-Rejected { Assert-InstallerUpgradeOwnedLocation 'C:\fixture\install' @{ InstallLocation = 'C:\customer\install' } } 'Unrelated registration accepted for uninstall.'
Assert-Rejected { Assert-InstallerUpgradeOwnedLocation 'C:\fixture\install' @{ InstallLocation = 'relative\install' } } 'Relative registration accepted for uninstall.'
Assert-InstallerUpgradeOwnedLocation 'C:\fixture\install' @{ InstallLocation = 'C:\fixture\install' }
$script:checks++
$owned = @{ InstallLocation = 'C:\fixture\install'; DisplayVersion = '0.3.3'; UninstallString = '"C:\fixture\install\Uninstall.exe"'; EstimatedSize = 42 }
# HKCU\SOFTWARE is shared on the actual Windows target. Two registry views of
# the same owned record are expected, not a second unrelated installation.
Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned ($owned.Clone()) $null $null
$script:checks++
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.4' $owned $owned $null $null } 'Shared record for the wrong expected version accepted.'
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $null $owned $null $null } 'Missing primary user registration accepted.'
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned $null $null $null } 'Missing shared user-view record accepted.'
foreach ($field in @('InstallLocation','DisplayVersion','UninstallString','EstimatedSize')) {
    $different = $owned.Clone(); $different[$field] = 'different'
    Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned $different $null $null } "Different shared-view value accepted: $field"
}
$differentType = $owned.Clone(); $differentType.EstimatedSize = '42'
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned $differentType $null $null } 'Different shared-view value type accepted.'
$differentNumericType = $owned.Clone(); $differentNumericType.EstimatedSize = [long]42
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned $differentNumericType $null $null } 'Different shared-view numeric value type accepted.'
$extra = $owned.Clone(); $extra.Unexpected = 'extra'
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned $extra $null $null } 'Extra shared-view registration value accepted.'
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned $owned $owned $null } 'Machine Registry64 registration accepted.'
Assert-Rejected { Assert-InstallerUpgradeRegistrationViews 'C:\fixture\install' '0.3.3' $owned $owned $null $owned } 'Machine Registry32 registration accepted.'
Write-Output "Installer upgrade preflight: $script:checks checks passed. Synthetic state tests only; no installer, registry write, profile write or trust mutation."
exit 0
