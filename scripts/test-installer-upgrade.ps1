param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$PortableArchive,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-upgrade-tools.ps1')
. (Join-Path $PSScriptRoot 'qualification-tools.ps1')
. (Join-Path $PSScriptRoot 'package-tools.ps1')
$hostEnvironment = @{}
foreach ($name in @('GITHUB_ACTIONS','RUNNER_ENVIRONMENT','RUNNER_OS','GITHUB_SERVER_URL','GITHUB_REPOSITORY','GITHUB_EVENT_NAME','GITHUB_SHA','GITHUB_RUN_ID')) {
    $hostEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
# This must precede network, filesystem fixture creation, or installer execution.
Assert-InstallerUpgradeHostedEnvironment $hostEnvironment
if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem -or -not [Environment]::Is64BitProcess) { throw '64-bit Windows process required.' }
if ($env:DISKSCOPE_HISTORY_DIR) { throw 'Unexpected history-directory override; use a clean hosted job.' }
$projectRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$version = Get-ReleaseVersion -ProjectRoot $projectRoot
if ($version -eq '0.3.2') { throw 'Candidate must be a different version from the fixed published 0.3.2 baseline.' }
$baselineVersion = '0.3.2'
$baselineInstallerHash = '807263cd323d6f154d4122723e7fe87e4999098246ca7b68fbfcf93cc4a03704'
$baselineArchiveHash = 'f15b5553d781e9f90d9764bb55490733c43d09d8b69504a67367c6ca53ea6874'
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\DiskScope'
$localData = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'DiskScope'
$roamingData = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'DiskScope'
$guardPaths = @(
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'DiskScope'),
    (Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'DiskScope.lnk'),
    (Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'DiskScope'),
    (Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'DiskScope.lnk'),
    (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs\DiskScope'),
    $localData, $roamingData
)
function Get-UpgradeRegistration([string]$Hive = 'CurrentUser', [string]$View = 'Registry64') {
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($Hive, $View)
    try {
        $key = $base.OpenSubKey($uninstallKey, $false)
        if ($null -eq $key) { return $null }
        try {
            $values = @{}
            foreach ($name in $key.GetValueNames()) { $values[$name] = $key.GetValue($name) }
            return $values
        } finally { $key.Dispose() }
    } finally { $base.Dispose() }
}
function Assert-CleanHostedInstallation {
    $registrations = @(
        foreach ($hive in @('CurrentUser','LocalMachine')) {
            foreach ($view in @('Registry64','Registry32')) {
                $registration = Get-UpgradeRegistration $hive $view
                if ($null -ne $registration) { [pscustomobject]@{ hive = $hive; view = $view; values = $registration } }
            }
        }
    )
    $existing = @(
        foreach ($path in $guardPaths) {
            Assert-NoReleaseReparsePath $path
            try { Get-Item -LiteralPath $path -Force -ErrorAction Stop }
            catch [System.Management.Automation.ItemNotFoundException] { }
        }
    )
    Assert-InstallerUpgradeCleanState $registrations $existing @(Get-Process -Name DiskScope -ErrorAction SilentlyContinue)
}
Assert-CleanHostedInstallation
$output = [IO.Path]::GetFullPath($OutputDirectory)
$allowed = [IO.Path]::GetFullPath((Join-Path $projectRoot 'target\hosted-qualification')).TrimEnd('\') + '\'
if (-not $output.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase)) { throw 'OutputDirectory must be new and within target\hosted-qualification.' }
Assert-NoReleaseReparsePath $output
if (Test-Path -LiteralPath $output) { throw 'OutputDirectory already exists; evidence must not be overwritten.' }
if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($output)) -PathType Container)) { throw 'Output parent must exist.' }
foreach ($name in @('Installer','PortableArchive')) {
    $path = (Resolve-Path -LiteralPath (Get-Variable -Name $name -ValueOnly)).ProviderPath
    Assert-NoReleaseReparsePath $path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'Candidate package must be a file.' }
    Set-Variable -Name $name -Value $path
}
New-Item -ItemType Directory -Path $output | Out-Null
$fixtureId = [guid]::NewGuid().ToString('N')
$fixture = Join-Path $output ('fixture-' + $fixtureId)
New-Item -ItemType Directory -Path $fixture | Out-Null
$installDirectory = Join-Path $fixture 'install with spaces'
if ($installDirectory -match '["\r\n]') { throw 'Unsafe NSIS argument path.' }
$installedExe = Join-Path $installDirectory 'DiskScope.exe'
$installedUninstaller = Join-Path $installDirectory 'Uninstall.exe'
$insideCanary = Join-Path $installDirectory 'unrelated user file.keep'
$profileCanary = Join-Path $localData ('installer-binary-rollback-' + $fixtureId + '.keep')
$canaryContent = 'Owned binary upgrade/rollback opaque canary ' + $fixtureId
$script:ownedUninstallerHash = $null
$script:checks = 0
$script:stages = [Collections.Generic.List[object]]::new()
$script:attemptedInstallation = $false
$profileCreated = $false
$started = [DateTime]::UtcNow.ToString('o')
$failure = $null
$cleanupFailure = $null
$status = 'failed'
function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }; $script:checks++
}
function Invoke-UpgradeSetup([string]$Path, [string]$Arguments) {
    # NSIS /D= and _?= consume the final unquoted remainder, including spaces.
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -WorkingDirectory $fixture -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(120000)) {
            $process.Kill(); $process.WaitForExit()
            throw 'Owned installer/uninstaller exceeded 120 seconds.'
        }
        $process.Refresh()
        Assert-True ($process.ExitCode -eq 0) "Installer/uninstaller exited $($process.ExitCode)."
    } finally { $process.Dispose() }
}
function Get-ValidatedArchive([string]$Archive, [string]$ExpectedVersion, [string]$Name) {
    $destination = Join-Path $fixture $Name
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    # Validate every entry lexically before extraction, even though the baseline
    # archive also has its independently fixed published digest.
    $archiveZip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        foreach ($entry in $archiveZip.Entries) {
            if ($entry.FullName -notlike "DiskScope-$ExpectedVersion-portable/*" -or $entry.FullName -match '[\\:]' -or
                @($entry.FullName.Split('/') | Where-Object { $_ -in @('.', '..') }).Count) { throw 'Unexpected/unsafe portable archive entry.' }
        }
    } finally { $archiveZip.Dispose() }
    [IO.Compression.ZipFile]::ExtractToDirectory($Archive, $destination)
    $root = Join-Path $destination "DiskScope-$ExpectedVersion-portable"
    $files = @(Assert-ReleasePayload -Directory $root -Version $ExpectedVersion)
    $hashes = @{}
    foreach ($file in $files) { $hashes[$file.FullName.Substring($root.Length + 1)] = (Get-FileHash -LiteralPath $file.FullName).Hash }
    return [pscustomobject]@{ version = $ExpectedVersion; root = $root; hashes = $hashes; executable_sha256 = $hashes['DiskScope.exe'] }
}
function Assert-OwnedCanaries {
    Assert-NoReleaseReparsePath $installDirectory
    Assert-NoReleaseReparsePath $localData
    foreach ($path in @($insideCanary, $profileCanary)) {
        Assert-True ((Get-Item -LiteralPath $path -Force).PSIsContainer -eq $false -and [IO.File]::ReadAllText($path) -ceq $canaryContent) "Owned canary changed: $path"
    }
    Assert-True (@(Get-ChildItem -LiteralPath $localData -Force).Count -eq 1) 'Unexpected profile data appeared; no history-format compatibility claim is allowed.'
    Assert-True (-not (Test-Path -LiteralPath $roamingData)) 'Unexpected roaming profile data appeared.'
}
function Assert-InstalledStage($Payload, [string]$StageName) {
    $registration = Get-UpgradeRegistration
    Assert-InstallerUpgradeOwnedLocation $installDirectory $registration
    # Establish cleanup ownership before outcome assertions. A diagnostic failure
    # must not prevent uninstall of this exact newly-created fixture. Cleanup
    # still rechecks the registered path and this byte-identical uninstaller.
    Assert-NoReleaseReparsePath $installedUninstaller
    Assert-True (Test-Path -LiteralPath $installedUninstaller -PathType Leaf) 'Owned uninstaller missing.'
    $script:ownedUninstallerHash = (Get-FileHash -LiteralPath $installedUninstaller).Hash
    $user32 = Get-UpgradeRegistration 'CurrentUser' 'Registry32'
    $machine64 = Get-UpgradeRegistration 'LocalMachine' 'Registry64'
    $machine32 = Get-UpgradeRegistration 'LocalMachine' 'Registry32'
    Assert-InstallerUpgradeRegistrationViews $installDirectory $Payload.version $registration $user32 $machine64 $machine32
    foreach ($relative in @($Payload.hashes.Keys)) {
        $installed = Join-Path $installDirectory $relative
        Assert-NoReleaseReparsePath $installed
        Assert-True ((Get-FileHash -LiteralPath $installed).Hash -ceq $Payload.hashes[$relative]) "Installed payload mismatch: $relative"
    }
    foreach ($file in @(Get-ReleaseFiles -Directory $installDirectory)) {
        $relative = $file.FullName.Substring($installDirectory.Length + 1)
        Assert-True ($Payload.hashes.ContainsKey($relative) -or $relative -in @('Uninstall.exe', 'INSTALLER_NOTICES.txt', 'unrelated user file.keep')) "Unexpected/stale installed file at $StageName`: $relative"
    }
    Assert-OwnedCanaries
    $reportPath = Join-Path $output ($StageName + '-cli.json')
    $process = Invoke-HelpdeskProcess $installedExe @('--scan', (Join-Path $fixture 'scan'), '--json', $reportPath, '--backend', 'normal', '--timeout', '30') -Headless -TimeoutSeconds 60 -ObserveResources
    Assert-True ($process.ExitCode -eq 0 -and -not $process.TimedOut) 'Installed stage CLI failed.'
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    Assert-True ((Get-DiskScopeReportVersion $report $Payload.version) -ceq $Payload.version -and $report.complete -and $report.files -eq 1 -and $report.logical_bytes -eq 11 -and $report.errors_total -eq 0) 'Installed stage version/known-answer report mismatch.'
    Assert-OwnedCanaries
    $script:stages.Add([pscustomobject]@{ id = $StageName; status = 'passed'; observed_at_utc = [DateTime]::UtcNow.ToString('o'); version = $Payload.version; executable_sha256 = (Get-FileHash -LiteralPath $installedExe).Hash.ToLowerInvariant(); expected_executable_sha256 = $Payload.executable_sha256.ToLowerInvariant(); payload_files_verified = $Payload.hashes.Count; registration_views = @{ current_user64 = $registration; current_user32 = $user32; machine64 = $machine64; machine32 = $machine32 }; cli = $process })
}
function Invoke-StrictOwnedUninstall {
    Assert-NoReleaseReparsePath $installDirectory
    Assert-InstallerUpgradeOwnedLocation $installDirectory (Get-UpgradeRegistration)
    Assert-True ($script:ownedUninstallerHash -and (Get-FileHash -LiteralPath $installedUninstaller).Hash -ceq $script:ownedUninstallerHash) 'Uninstaller no longer matches the last validated stage; preserving fixture.'
    $copy = Join-Path $fixture ('uninstaller-' + [guid]::NewGuid().ToString('N') + '.exe')
    [IO.File]::Copy($installedUninstaller, $copy, $false)
    Assert-True ((Get-FileHash -LiteralPath $copy).Hash -ceq $script:ownedUninstallerHash) 'Uninstaller copy mismatch.'
    Invoke-UpgradeSetup $copy "/S _?=$installDirectory"
}
function Receive-PinnedBaseline([string]$Name, [string]$Hash) {
    $path = Join-Path $fixture $Name
    if (Test-Path -LiteralPath $path) { throw 'Baseline download destination already exists.' }
    $url = 'https://github.com/davidwilliams-crypto/DiskScope/releases/download/v0.3.2/' + $Name
    & curl.exe --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --connect-timeout 20 --max-time 180 --max-filesize 100000000 --output $path $url
    if ($LASTEXITCODE -ne 0) { throw 'Published baseline download failed; no installer executed.' }
    Assert-True ((Get-FileHash -LiteralPath $path).Hash -ieq $Hash) 'Published 0.3.2 artifact hash mismatch; no installer executed.'
    return $path
}
try {
    $baselineInstaller = Receive-PinnedBaseline 'DiskScope-0.3.2-windows-x64-setup.exe' $baselineInstallerHash
    $baselineArchive = Receive-PinnedBaseline 'DiskScope-0.3.2-windows-x64-portable.zip' $baselineArchiveHash
    $baseline = Get-ValidatedArchive $baselineArchive $baselineVersion 'baseline-portable'
    $candidate = Get-ValidatedArchive $PortableArchive $version 'candidate-portable'
    New-Item -ItemType Directory -Path (Join-Path $fixture 'scan') | Out-Null
    Write-QualificationNewFile (Join-Path $fixture 'scan/known.txt') 'known bytes'
    # Recheck immediately before creating profile state/starting setup.
    Assert-CleanHostedInstallation
    New-Item -ItemType Directory -Path $localData | Out-Null
    $profileCreated = $true
    Write-QualificationNewFile $profileCanary $canaryContent
    $script:attemptedInstallation = $true
    Invoke-UpgradeSetup $baselineInstaller "/S /NoShortcuts /D=$installDirectory"
    Write-QualificationNewFile $insideCanary $canaryContent
    Assert-InstalledStage $baseline 'baseline-install'
    Invoke-UpgradeSetup $Installer '/S /NoShortcuts'
    Assert-InstalledStage $candidate 'candidate-upgrade'
    Invoke-UpgradeSetup $baselineInstaller '/S /NoShortcuts'
    Assert-InstalledStage $baseline 'baseline-binary-rollback'
    Invoke-StrictOwnedUninstall
    $script:attemptedInstallation = $false
    Assert-True ($null -eq (Get-UpgradeRegistration)) 'Uninstall registration remains.'
    Assert-True (-not (Test-Path -LiteralPath $installedExe) -and -not (Test-Path -LiteralPath $installedUninstaller)) 'Owned application/uninstaller remains after uninstall.'
    foreach ($relative in @($baseline.hashes.Keys)) { Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory $relative))) "Baseline payload remains after uninstall: $relative" }
    Assert-OwnedCanaries
    foreach ($path in @($guardPaths | Where-Object { $_ -notin @($localData, $roamingData) })) { Assert-True (-not (Test-Path -LiteralPath $path)) "Unexpected shortcut/default install path: $path" }
    $script:stages.Add([pscustomobject]@{ id = 'uninstall-after-binary-rollback'; status = 'passed'; observed_at_utc = [DateTime]::UtcNow.ToString('o'); canaries_preserved = $true })
    $status = 'passed'
} catch { $failure = $_.Exception.Message }
finally {
    try {
        if ($script:attemptedInstallation -and $script:ownedUninstallerHash -and (Test-Path -LiteralPath $installedUninstaller)) { Invoke-StrictOwnedUninstall }
        if ($profileCreated) {
            # Never recursively clean a real profile. Remove only our exact
            # unchanged canary, then its proven-empty, initially absent parent.
            Assert-NoReleaseReparsePath $profileCanary
            if ([IO.File]::ReadAllText($profileCanary) -cne $canaryContent) { throw 'Profile canary changed; preserving it.' }
            if (@(Get-ChildItem -LiteralPath $localData -Force).Count -ne 1) { throw 'Unexpected profile files; preserving directory.' }
            Remove-Item -LiteralPath $profileCanary
            [IO.Directory]::Delete($localData, $false)
        }
        Assert-CleanHostedInstallation
    } catch { $cleanupFailure = $_.Exception.Message; $status = 'failed' }
    $record = [ordered]@{
        schema_version = 1; status = $status; started_at_utc = $started; finished_at_utc = [DateTime]::UtcNow.ToString('o'); commit = $env:GITHUB_SHA; checks_passed = $script:checks
        baseline = @{ version = $baselineVersion; installer_sha256 = $baselineInstallerHash; portable_sha256 = $baselineArchiveHash }
        candidate = @{ version = $version; installer_sha256 = (Get-FileHash -LiteralPath $Installer).Hash.ToLowerInvariant(); portable_sha256 = (Get-FileHash -LiteralPath $PortableArchive).Hash.ToLowerInvariant() }
        stages = @($script:stages); failure = $failure; cleanup_failure = $cleanupFailure
        scope = 'Actual hosted Windows Server binary upgrade 0.3.2 to candidate, binary rollback to 0.3.2, exact required payload/CLI version, opaque file/profile canary preservation, uninstall. No runtime history/snapshot-format, client-OS, remote, UAC, publisher or fleet qualification is inferred. Fixture retained; only exact owned profile canary/empty parent cleaned.'
    }
    Write-QualificationNewFile (Join-Path $output 'binary-upgrade-rollback.json') ($record | ConvertTo-Json -Depth 20)
}
if ($status -ne 'passed') { throw "Hosted binary upgrade/rollback failed: $failure Cleanup: $cleanupFailure" }
Write-Output "Hosted binary upgrade/rollback: $script:checks checks passed. Binary/file preservation scope only. Evidence: $output"
$global:LASTEXITCODE = 0
