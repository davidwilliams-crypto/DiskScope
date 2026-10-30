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
