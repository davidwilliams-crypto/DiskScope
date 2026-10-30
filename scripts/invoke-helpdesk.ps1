<#
.SYNOPSIS
Stages a verified DiskScope executable into a new directory and launches a private session.
.DESCRIPTION
For ScreenConnect, deliver the approved executable and these scripts using the managed
toolbox first. Obtain ExpectedSha256 independently. No downloads, elevation, installer,
customer-file cleanup, or changes to machine trust are performed. Use -Mode Scan for
SYSTEM and command tools. StageDirectory must be new; its parent must already be secured.
Wrapper exit codes: 20 preflight/staging/context, 21 launch failure. Otherwise forwards
the application code (0 complete, 1 failed/report I/O, 2 usage, 3 timeout, 4 partial).
#>
param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$ExpectedSha256,
    [Parameter(Mandatory)][string]$StageDirectory,
    [ValidateSet('Scan', 'Helpdesk', 'NoHistory')][string]$Mode = 'Helpdesk',
    [string]$Root,
    [string]$ReportPath,
    [ValidateRange(1, 86400)][int]$TimeoutSeconds = 300,
    [string]$ApprovedSignerThumbprint
)
$ErrorActionPreference = 'Stop'
# A nonzero application status is a result to forward, not a launch exception.
$PSNativeCommandUseErrorActionPreference = $false
. (Join-Path $PSScriptRoot 'helpdesk-tools.ps1')
try {
    if ($env:OS -ne 'Windows_NT') { throw 'This deployment wrapper supports Windows only.' }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    try { $isSystemAccount = $identity.User.Value -eq 'S-1-5-18' } finally { $identity.Dispose() }
    Assert-HelpdeskGuiContext $Mode $isSystemAccount ([Environment]::UserInteractive) ([Diagnostics.Process]::GetCurrentProcess().SessionId)
    if ($Root) { $Root = [IO.Path]::GetFullPath($Root) }
    if ($ReportPath) {
        $ReportPath = Assert-HelpdeskReportDestination $ReportPath
    }
    $applicationArgs = @(Get-HelpdeskArguments $Mode $Root $ReportPath $TimeoutSeconds)
    $stagedExecutable = New-HelpdeskStage $Executable $ExpectedSha256 $StageDirectory $Mode $ApprovedSignerThumbprint
} catch { [Console]::Error.WriteLine("DiskScope preflight: $($_.Exception.Message)"); exit 20 }
try {
    $result = Invoke-HelpdeskProcess $stagedExecutable $applicationArgs -Headless:($Mode -eq 'Scan')
    # Console writes preserve native stderr without turning it into a terminating
    # Windows PowerShell NativeCommandError under ErrorActionPreference=Stop.
    if ($result.StandardOutput) { [Console]::Out.Write($result.StandardOutput) }
    if ($result.StandardError) { [Console]::Error.Write($result.StandardError) }
    exit $result.ExitCode
} catch { [Console]::Error.WriteLine("DiskScope launch: $($_.Exception.Message)"); exit 21 }
