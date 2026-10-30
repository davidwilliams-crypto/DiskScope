# Pure assertions. Importing this file never reads/writes installer state.
function Assert-InstallerUpgradeHostedEnvironment {
    param([hashtable]$Environment)
    if ($Environment.GITHUB_ACTIONS -cne 'true' -or
        $Environment.RUNNER_ENVIRONMENT -cne 'github-hosted' -or
        $Environment.RUNNER_OS -cne 'Windows' -or
        $Environment.GITHUB_SERVER_URL -cne 'https://github.com' -or
        $Environment.GITHUB_REPOSITORY -cne 'davidwilliams-crypto/DiskScope' -or
        $Environment.GITHUB_EVENT_NAME -notin @('push', 'workflow_dispatch') -or
        $Environment.GITHUB_SHA -notmatch '^[a-f0-9]{40}$' -or
        $Environment.GITHUB_RUN_ID -notmatch '^[0-9]+$') {
        throw 'Binary upgrade/rollback execution requires the intended disposable GitHub-hosted Windows workflow. Local/self-hosted installation is forbidden.'
    }
}

function Assert-InstallerUpgradeCleanState {
    param([object[]]$Registrations, [object[]]$ExistingPaths, [object[]]$RunningProcesses)
    if (@($Registrations | Where-Object { $null -ne $_ }).Count -gt 0 -or
        @($ExistingPaths | Where-Object { $null -ne $_ }).Count -gt 0 -or
        @($RunningProcesses | Where-Object { $null -ne $_ }).Count -gt 0) {
        throw 'Refusing binary upgrade/rollback: an installation, shortcut, data/default directory or running DiskScope process already exists. Nothing will be replaced or removed.'
    }
}

function Assert-InstallerUpgradeOwnedLocation {
    param([string]$Expected, $Registration)
    if ($null -eq $Registration -or -not $Registration.InstallLocation -or
        -not [IO.Path]::IsPathRooted([string]$Registration.InstallLocation) -or
        [IO.Path]::GetFullPath([string]$Registration.InstallLocation).TrimEnd('\') -ine [IO.Path]::GetFullPath($Expected).TrimEnd('\')) {
        throw 'Refusing uninstaller execution: registration does not identify the exact owned fixture installation.'
    }
}

function Assert-InstallerUpgradeRegistrationViews {
    param([string]$Expected, [string]$Version, [hashtable]$CurrentUser64, [hashtable]$CurrentUser32,
        [hashtable]$LocalMachine64, [hashtable]$LocalMachine32)
    Assert-InstallerUpgradeOwnedLocation $Expected $CurrentUser64
    if ($CurrentUser64.DisplayVersion -cne $Version) { throw 'Registered version differs from the expected owned stage.' }
    if ($null -ne $LocalMachine64 -or $null -ne $LocalMachine32) { throw 'Unexpected machine installation registration; this test installs per user only.' }
    # HKCU\SOFTWARE and its Uninstall subkey are shared, so both logical views
    # must describe the identical owned record. Never accept a different record.
    # https://learn.microsoft.com/en-us/windows/win32/winprog64/shared-registry-keys
    if ($null -eq $CurrentUser32) { throw 'Expected the shared current-user registration in both logical views.' }
    $primary = [ordered]@{}
    $shared = [ordered]@{}
    foreach ($name in @($CurrentUser64.Keys | Sort-Object)) {
        $value = $CurrentUser64[$name]
        $primary[$name] = [ordered]@{ type = $(if ($null -eq $value) { 'null' } else { $value.GetType().FullName }); value = $value }
    }
    foreach ($name in @($CurrentUser32.Keys | Sort-Object)) {
        $value = $CurrentUser32[$name]
        $shared[$name] = [ordered]@{ type = $(if ($null -eq $value) { 'null' } else { $value.GetType().FullName }); value = $value }
    }
    if (($primary | ConvertTo-Json -Compress -Depth 8) -cne ($shared | ConvertTo-Json -Compress -Depth 8)) {
        throw 'Current-user logical registry views differ; expected the identical complete owned registration record.'
    }
}
