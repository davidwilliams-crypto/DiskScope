param(
    [string]$Installer,
    [string]$PortableArchive,
    [switch]$NoShortcuts,
    [switch]$DefaultDirectory
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-tools.ps1')
. (Join-Path $PSScriptRoot 'package-tools.ps1')
$projectRoot = Split-Path -Parent $PSScriptRoot
$version = Get-ReleaseVersion -ProjectRoot $projectRoot
if (-not $Installer) { $Installer = "dist\releases\$version\DiskScope-$version-windows-x64-setup.exe" }
if (-not $PortableArchive) { $PortableArchive = "dist\releases\$version\DiskScope-$version-windows-x64-portable.zip" }
foreach ($name in @('Installer', 'PortableArchive')) {
    $path = Get-Variable -Name $name -ValueOnly
    if (-not [IO.Path]::IsPathRooted($path)) { $path = Join-Path $projectRoot $path }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing $name artifact: $path. Run scripts/package-release.ps1 first." }
    Set-Variable -Name $name -Value (Resolve-Path -LiteralPath $path).ProviderPath
}
if (-not [Environment]::Is64BitOperatingSystem -or $env:OS -ne 'Windows_NT') { throw 'Installer smoke tests require 64-bit Windows.' }

$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\DiskScope'
$fixtureId = [guid]::NewGuid().ToString('N')
$fixtureRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'target\installer-tests'))
$fixture = Join-Path $fixtureRoot $fixtureId
$defaultInstallDirectory = [IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs\DiskScope'))
$installDirectory = if ($DefaultDirectory) { $defaultInstallDirectory } else { Join-Path $fixture 'install with spaces' }
$installedExe = Join-Path $installDirectory 'DiskScope.exe'
$installedUninstaller = Join-Path $installDirectory 'Uninstall.exe'
$uninstallerCopy = Join-Path $fixture 'test uninstaller.exe'
$startMenuDirectory = Join-Path ([Environment]::GetFolderPath('Programs')) 'DiskScope'
$startMenuShortcut = Join-Path $startMenuDirectory 'DiskScope.lnk'
$desktopShortcut = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'DiskScope.lnk'
$shortcutGuardPaths = @(
    $startMenuDirectory,
    $desktopShortcut,
    (Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'DiskScope'),
    (Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'DiskScope.lnk')
)
$dataDirectories = @(
    (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'DiskScope'),
    (Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'DiskScope')
)
if ($env:DISKSCOPE_HISTORY_DIR) { $dataDirectories += [IO.Path]::GetFullPath($env:DISKSCOPE_HISTORY_DIR) }
$dataDirectories = @($dataDirectories | Sort-Object -Unique)
$script:checks = 0
$installationAttempted = $false
$fixtureCreated = $false
$defaultPathAbsentBefore = $false
$completed = $false

function Assert-True([bool]$Condition, [string]$Description) {
    if (-not $Condition) { throw $Description }
    $script:checks++
}

function Get-InstallationRegistration {
    param(
        [Microsoft.Win32.RegistryHive]$Hive = [Microsoft.Win32.RegistryHive]::CurrentUser,
        [Microsoft.Win32.RegistryView]$View = [Microsoft.Win32.RegistryView]::Registry64
    )
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($Hive, $View)
    try {
        $key = $base.OpenSubKey($uninstallKey, $false)
        if ($null -eq $key) { return $null }
        try {
            $values = @{}
            foreach ($name in $key.GetValueNames()) { $values[$name] = $key.GetValue($name) }
            return [pscustomobject]@{ Hive = $Hive; View = $View; Values = $values }
        } finally { $key.Dispose() }
    } finally { $base.Dispose() }
}

function Assert-NoExistingInstallation {
    foreach ($hive in @('CurrentUser', 'LocalMachine')) {
        foreach ($view in @('Registry64', 'Registry32')) {
            if ($null -ne (Get-InstallationRegistration -Hive $hive -View $view)) {
                throw "Refusing to test: DiskScope is already registered in $hive/$view. Use a clean Windows account or uninstall it yourself first."
            }
        }
    }
    foreach ($path in $shortcutGuardPaths) {
        if (Test-Path -LiteralPath $path) { throw "Refusing to test: an existing DiskScope shortcut or Start menu folder would be affected: $path" }
    }
}

function Get-ExistingInstallerTestPath([string]$Path) {
    # Get-Item also inspects dangling links; Test-Path alone may treat one as
    # absent. Permission and other read failures must abort instead of passing.
    try { return Get-Item -LiteralPath $Path -Force -ErrorAction Stop }
    catch [System.Management.Automation.ItemNotFoundException] { return $null }
}

function Assert-DefaultInstallPathSafety {
    $expected = [IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs\DiskScope'))
    $resolved = [IO.Path]::GetFullPath($installDirectory)
    if ($resolved -ine $expected -or $resolved -ine $defaultInstallDirectory -or
        [IO.Path]::GetFileName($resolved) -cne 'DiskScope') {
        throw 'Refusing default-folder access: the destination is not the exact current-user Programs\DiskScope directory.'
    }
    $ancestor = [IO.DirectoryInfo]::new($resolved)
    while ($null -ne $ancestor) {
        $item = Get-ExistingInstallerTestPath -Path $ancestor.FullName
        if ($null -ne $item) {
            if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw "Refusing default-folder access through a file or reparse point: $($ancestor.FullName)"
            }
        }
        $ancestor = $ancestor.Parent
    }
}

function Assert-DefaultInstallDestinationAbsent {
    Assert-DefaultInstallPathSafety
    if ($null -ne (Get-ExistingInstallerTestPath -Path $defaultInstallDirectory)) {
        throw "Refusing default-folder test: a file or directory already exists at '$defaultInstallDirectory', even if it is unregistered."
    }
}

function Get-UserDataSnapshot {
    # Read only. Never create, move, restore, or delete real history/preferences.
    # Record reparse points without following them outside the chosen data roots.
    $rows = [Collections.Generic.List[string]]::new()
    foreach ($root in $dataDirectories) {
        if (-not (Test-Path -LiteralPath $root)) { $rows.Add("absent|$root"); continue }
        $pending = [Collections.Generic.Stack[string]]::new()
        $pending.Push($root)
        while ($pending.Count -gt 0) {
            $item = Get-Item -LiteralPath $pending.Pop() -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                $rows.Add("link|$($item.FullName)|$($item.Attributes)|$($item.Target -join '|')")
            } elseif ($item.PSIsContainer) {
                $rows.Add("directory|$($item.FullName)")
                foreach ($child in Get-ChildItem -LiteralPath $item.FullName -Force) { $pending.Push($child.FullName) }
            } else {
                $hash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash
                $rows.Add("file|$($item.FullName)|$($item.Length)|$($item.LastWriteTimeUtc.Ticks)|$hash")
            }
        }
    }
    return @($rows | Sort-Object)
}

function Invoke-TestProcess {
    param([string]$Executable, [string]$Arguments, [string]$Description, [switch]$ExpectFailure)
    # Arguments is deliberately one string: NSIS /D= and _?= consume the entire
    # unquoted remainder of the command line, including spaces in the path.
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -WorkingDirectory $fixture -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(120000)) {
            $process.Kill()
            $process.WaitForExit()
            throw "$Description timed out after 120 seconds."
        }
        $process.Refresh()
        $code = $process.ExitCode
        if ($ExpectFailure) { Assert-True ($code -ne 0) "$Description unexpectedly succeeded." }
        else { Assert-True ($code -eq 0) "$Description failed with exit code $code." }
    } finally { $process.Dispose() }
}

function Assert-Registration {
    $registration = Get-InstallationRegistration
    Assert-True ($null -ne $registration) 'The per-user 64-bit uninstall registration is missing.'
    $values = $registration.Values
    Assert-True ($values['DisplayName'] -ceq 'DiskScope') 'Unexpected installed display name.'
    Assert-True ($values['DisplayVersion'] -ceq $version) 'Installed version does not match Cargo.toml.'
    Assert-True ($values['InstallLocation'] -ieq $installDirectory) 'InstallLocation does not match the expected installation directory.'
    Assert-True ($values['UninstallString'] -ceq ('"' + $installedUninstaller + '"')) 'UninstallString must quote the complete executable path.'
    Assert-True ($values['QuietUninstallString'] -ceq ('"' + $installedUninstaller + '" /S')) 'QuietUninstallString must quote the complete executable path.'
    Assert-True ($values['NoModify'] -eq 1 -and $values['NoRepair'] -eq 1) 'Uninstall registration must disable unsupported modify/repair actions.'
    Assert-True ($values['EstimatedSize'] -gt 0) 'Uninstall registration must include the installed size.'
    foreach ($view in @('Registry64', 'Registry32')) {
        Assert-True ($null -eq (Get-InstallationRegistration -Hive LocalMachine -View $view)) 'A per-user install must not register in HKLM.'
    }
    Assert-True (-not (Test-Path -LiteralPath $desktopShortcut)) 'The optional desktop shortcut must be off by default.'
    if ($NoShortcuts) {
        Assert-True (-not (Test-Path -LiteralPath $startMenuDirectory)) '/NoShortcuts must not create a Start menu folder.'
        Assert-True (-not $values.ContainsKey('StartMenuShortcut')) '/NoShortcuts must not register a Start menu shortcut.'
    } else {
        Assert-True (Test-Path -LiteralPath $startMenuShortcut -PathType Leaf) 'The default Start menu shortcut is missing.'
        Assert-True ($values['StartMenuShortcut'] -eq 1) 'The installer must record ownership of its Start menu shortcut.'
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $null
        try {
            $shortcut = $shell.CreateShortcut($startMenuShortcut)
            Assert-True ($shortcut.TargetPath -ieq $installedExe) 'The Start menu shortcut points to the wrong executable.'
            Assert-True ([string]::IsNullOrEmpty($shortcut.Arguments)) 'The Start menu shortcut must not start an automatic scan.'
        } finally {
            if ($null -ne $shortcut) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) }
            [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
        }
    }
}

function Assert-InstalledPayload {
    # Assert-ReleasePayload has already validated manifest paths and ZIP hashes.
    # Compare SHA256SUMS.txt itself as well as every file covered by that manifest.
    foreach ($file in $portableFiles) {
        $relative = $file.FullName.Substring($portableRoot.Length + 1)
        $installed = Join-Path $installDirectory $relative
        Assert-True (Test-Path -LiteralPath $installed -PathType Leaf) "Installed payload file is missing: $relative"
        Assert-True ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ceq $portableHashes[$relative]) "Installed payload differs from portable ZIP: $relative"
    }
    Assert-True (Test-Path -LiteralPath $installedUninstaller -PathType Leaf) 'The installed uninstaller is missing.'
    Assert-True (Test-Path -LiteralPath (Join-Path $installDirectory 'INSTALLER_NOTICES.txt') -PathType Leaf) 'Installer redistribution notices are missing.'
}

function Invoke-OwnedUninstaller {
    param([switch]$ExpectFailure)
    if ($DefaultDirectory) { Assert-DefaultInstallPathSafety }
    $registration = Get-InstallationRegistration
    if ($null -ne $registration -and $registration.Values['InstallLocation'] -ine $installDirectory) {
        throw 'Refusing cleanup: DiskScope registration no longer belongs to this fixture.'
    }
    # Run an identical copy outside INSTDIR. _?= prevents NSIS from relocating
    # itself, so process exit means uninstall finished and the installed
    # Uninstall.exe can be deleted without deleting the running process image.
    Copy-Item -LiteralPath $installedUninstaller -Destination $uninstallerCopy -Force
    Assert-True ((Get-FileHash -LiteralPath $uninstallerCopy).Hash -ceq (Get-FileHash -LiteralPath $installedUninstaller).Hash) 'Uninstaller test copy must be byte-identical.'
    Invoke-TestProcess -Executable $uninstallerCopy -Arguments "/S _?=$installDirectory" -Description 'Silent uninstall' -ExpectFailure:$ExpectFailure
}

function Remove-OwnedDefaultDirectory {
    if (-not $defaultPathAbsentBefore -or -not $installationAttempted) {
        throw 'Refusing default-folder cleanup without a confirmed absent destination before this test installed it.'
    }
    Assert-DefaultInstallPathSafety
    if (-not (Test-Path -LiteralPath $installDirectory)) { return }
    # This path is outside the workspace. Never recursively delete it: remove
    # only this test's exact sentinel and then directories proven to be empty.
    @(Get-ReleaseFiles -Directory $installDirectory) | Out-Null
    $sentinelDirectory = Join-Path $installDirectory 'user files'
    $expectedSentinel = Join-Path $sentinelDirectory 'keep this.txt'
    if ($insideSentinel -ine $expectedSentinel) { throw 'Refusing cleanup of an unexpected sentinel path.' }
    if (Test-Path -LiteralPath $insideSentinel) {
        $sentinel = Get-Item -LiteralPath $insideSentinel -Force
        if ($sentinel.PSIsContainer -or [IO.File]::ReadAllText($insideSentinel) -cne $sentinelContent) {
            throw "The test sentinel changed; preserving '$installDirectory'."
        }
        Remove-Item -LiteralPath $insideSentinel -Force
    }
    foreach ($directory in @($sentinelDirectory, $installDirectory)) {
        if (Test-Path -LiteralPath $directory) {
            if (@(Get-ChildItem -LiteralPath $directory -Force).Count -ne 0) {
                throw "Unexpected files remain; preserving '$directory'. No recursive cleanup is allowed outside the workspace."
            }
            [IO.Directory]::Delete($directory, $false)
        }
    }
}

function Remove-OwnedFixture {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $expected = [IO.Path]::GetFullPath((Join-Path $fixtureRoot $fixtureId))
    if ($resolved -ine $expected -or [IO.Path]::GetDirectoryName($resolved) -ine $fixtureRoot -or $fixtureId -notmatch '^[a-f0-9]{32}$') {
        throw 'Refusing cleanup outside the unique owned installer fixture.'
    }
    # Reject redirected ancestors and descendants before the one recursive delete.
    $ancestor = Get-Item -LiteralPath $resolved -Force
    while ($null -ne $ancestor) {
        if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing fixture cleanup through a reparse point: $($ancestor.FullName)" }
        $ancestor = $ancestor.Parent
    }
    @(Get-ReleaseFiles -Directory $resolved) | Out-Null
    Remove-Item -LiteralPath $resolved -Recurse -Force
}

Assert-NoExistingInstallation
if ($DefaultDirectory) { Assert-DefaultInstallDestinationAbsent }
$dataBefore = @(Get-UserDataSnapshot)
try {
    if (Test-Path -LiteralPath $fixture) { throw 'The unique fixture already exists.' }
    New-Item -ItemType Directory -Path $fixture -Force | Out-Null
    $fixtureCreated = $true
    if ($installDirectory -match '["\r\n]') { throw 'The fixture path cannot be represented safely in NSIS arguments.' }
    $extracted = Join-Path $fixture 'extracted portable'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($PortableArchive, $extracted)
    $portableRoot = Join-Path $extracted "DiskScope-$version-portable"
    Assert-True (@(Get-ChildItem -LiteralPath $extracted -Force).Count -eq 1) 'The portable ZIP must contain a single versioned root directory.'
    $portableFiles = @(Assert-ReleasePayload -Directory $portableRoot -Version $version)
    $portableHashes = @{}
    foreach ($file in $portableFiles) {
        $portableHashes[$file.FullName.Substring($portableRoot.Length + 1)] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    }
    $outsideSentinel = Join-Path $fixture 'outside-install.keep'
    $insideSentinel = Join-Path $installDirectory 'user files\keep this.txt'
    $sentinelContent = 'Owned smoke-test sentinel ' + $fixtureId
    [IO.File]::WriteAllText($outsideSentinel, $sentinelContent)
    $reinstallArguments = '/S'
    if ($NoShortcuts) { $reinstallArguments += ' /NoShortcuts' }
    $arguments = $reinstallArguments
    if (-not $DefaultDirectory) { $arguments += " /D=$installDirectory" }
    # Recheck immediately before running setup, after archive validation.
    Assert-NoExistingInstallation
    if ($DefaultDirectory) {
        Assert-DefaultInstallDestinationAbsent
        $defaultPathAbsentBefore = $true
    }
    $installationAttempted = $true
    Invoke-TestProcess -Executable $Installer -Arguments $arguments -Description 'Silent installation'
    Assert-Registration
    Assert-InstalledPayload
    New-Item -ItemType Directory -Path (Split-Path -Parent $insideSentinel) | Out-Null
    [IO.File]::WriteAllText($insideSentinel, $sentinelContent)

    $otherInstallDirectory = Join-Path $fixture 'refused move with spaces'
    Invoke-TestProcess -Executable $Installer -Arguments "/S /NoShortcuts /D=$otherInstallDirectory" -Description 'Installation move refusal' -ExpectFailure
    Assert-True (-not (Test-Path -LiteralPath $otherInstallDirectory)) 'A refused move must not create the alternate installation directory.'
    Assert-Registration
    Assert-InstalledPayload
    Assert-True ([IO.File]::ReadAllText($insideSentinel) -ceq $sentinelContent) 'A refused installation move changed an unknown file.'

    # CLI mode neither launches the GUI nor writes preference/history state.
    $scanRoot = Join-Path $fixture 'scan fixture'
    New-Item -ItemType Directory -Path (Join-Path $scanRoot 'nested\empty') -Force | Out-Null
    [IO.File]::WriteAllBytes((Join-Path $scanRoot 'one.bin'), [byte[]]@(1, 2, 3))
    [IO.File]::WriteAllBytes((Join-Path $scanRoot 'nested\two.bin'), [byte[]]@(4, 5, 6, 7, 8))
    [IO.File]::WriteAllBytes((Join-Path $scanRoot 'zero.bin'), [byte[]]@())
    $reportPath = Join-Path $fixture 'installed scan.json'
    Invoke-TestProcess -Executable $installedExe -Arguments ('--scan "' + $scanRoot + '" --json "' + $reportPath + '" --backend normal --timeout 30') -Description 'Installed CLI scan'
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    Assert-True ((Get-DiskScopeReportVersion -Report $report -ExpectedVersion $version) -ceq $version) 'CLI report version mismatch.'
    Assert-True ($report.complete -and $report.errors_total -eq 0) 'The installed CLI scan was incomplete.'
    Assert-True ($report.files -eq 3 -and $report.folders -eq 2 -and $report.logical_bytes -eq 8) 'Installed CLI scan must report three files, two child folders, and eight logical bytes.'

    # Omitting /D must reuse the registered install location, including spaces.
    Invoke-TestProcess -Executable $Installer -Arguments $reinstallArguments -Description 'Idempotent reinstall at remembered location'
    Assert-Registration
    Assert-InstalledPayload
    Assert-True ([IO.File]::ReadAllText($insideSentinel) -ceq $sentinelContent) 'Reinstall changed an unknown file inside the program directory.'

    # Hold a read handle that denies write/delete access, without starting GUI.
    $lockedExe = [IO.File]::Open($installedExe, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Invoke-TestProcess -Executable $Installer -Arguments $reinstallArguments -Description 'Locked executable update refusal' -ExpectFailure
        Assert-Registration
        Assert-InstalledPayload
        Assert-True ([IO.File]::ReadAllText($insideSentinel) -ceq $sentinelContent) 'Rejected update changed an unknown file.'
        Invoke-OwnedUninstaller -ExpectFailure
        Assert-Registration
        Assert-InstalledPayload
        Assert-True ([IO.File]::ReadAllText($insideSentinel) -ceq $sentinelContent) 'Rejected uninstall changed an unknown file.'
    } finally { $lockedExe.Dispose() }

    Invoke-OwnedUninstaller
    Assert-True ($null -eq (Get-InstallationRegistration)) 'Uninstall registration was not removed.'
    foreach ($file in $portableFiles) {
        $relative = $file.FullName.Substring($portableRoot.Length + 1)
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory $relative))) "Uninstall left a packaged file: $relative"
    }
    Assert-True (-not (Test-Path -LiteralPath $installedUninstaller)) 'Uninstall.exe was not removed.'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory 'INSTALLER_NOTICES.txt'))) 'Installer notices were not removed.'
    foreach ($path in $shortcutGuardPaths) { Assert-True (-not (Test-Path -LiteralPath $path)) "Uninstall left a shortcut or Start menu directory: $path" }
    Assert-True ([IO.File]::ReadAllText($insideSentinel) -ceq $sentinelContent) 'Uninstall removed or changed an unknown file inside the program directory.'
    Assert-True ([IO.File]::ReadAllText($outsideSentinel) -ceq $sentinelContent) 'Installer lifecycle changed a file outside the program directory.'
    $completed = $true
} finally {
    $cleanupError = $null
    try {
        if ($installationAttempted -and (Test-Path -LiteralPath $installedUninstaller -PathType Leaf)) { Invoke-OwnedUninstaller }
        if ($installationAttempted) { Assert-NoExistingInstallation }
        if ($DefaultDirectory -and $installationAttempted) { Remove-OwnedDefaultDirectory }
        if ($fixtureCreated) { Remove-OwnedFixture }
    } catch {
        $cleanupError = $_
        Write-Warning "Installer test cleanup failed. Fixture retained at '$fixture'; installation path is '$installDirectory'. Registry and shortcuts are removed only by the production uninstaller. $($_.Exception.Message)"
    }
    $dataAfter = @(Get-UserDataSnapshot)
    Assert-True (@(Compare-Object -ReferenceObject $dataBefore -DifferenceObject $dataAfter).Count -eq 0) 'Real DiskScope preferences/history changed during the installer test.'
    if ($null -ne $cleanupError) { throw $cleanupError }
}
if ($completed) {
    $shortcutMode = if ($NoShortcuts) { 'disabled' } else { 'default Start menu' }
    $directoryMode = if ($DefaultDirectory) { 'current-user default' } else { 'custom path with spaces' }
    Write-Output "Installer lifecycle: $script:checks checks passed (shortcuts: $shortcutMode; destination: $directoryMode). Install, CLI scan, reinstall, locked-file refusal, uninstall, payload parity, and user-data preservation verified."
}
