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
Write-Output "Installer upgrade preflight: $script:checks checks passed. Synthetic state tests only; no installer, registry write, profile write or trust mutation."
exit 0
