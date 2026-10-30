# Shared functions; no network, process launch, signing or trust-store mutation on import.
. (Join-Path $PSScriptRoot 'release-tools.ps1')

function ConvertTo-HelpdeskNativeArgument {
    param([AllowEmptyString()][string]$Argument)
    # Windows argv quoting: double backslashes before a quote or the closing
    # delimiter. Works with ProcessStartInfo.Arguments on both .NET Framework
    # (Windows PowerShell 5.1) and modern .NET (PowerShell 7).
    $escaped = [regex]::Replace($Argument, '(\\*)"', '$1$1\"')
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}

function Invoke-HelpdeskProcess {
    param([string]$Executable, [string[]]$Arguments, [switch]$Headless)
    $process = [Diagnostics.Process]::new()
    try {
        $process.StartInfo.FileName = $Executable
        $process.StartInfo.Arguments = (@($Arguments | ForEach-Object { ConvertTo-HelpdeskNativeArgument $_ }) -join ' ')
        $process.StartInfo.UseShellExecute = $false
        $process.StartInfo.CreateNoWindow = [bool]$Headless
        $process.StartInfo.RedirectStandardOutput = [bool]$Headless
        $process.StartInfo.RedirectStandardError = [bool]$Headless
        if ($Headless) {
            # Rust writes UTF-8 when its standard streams are redirected pipes.
            $process.StartInfo.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
            $process.StartInfo.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
        }
        if (-not $process.Start()) { throw 'Windows did not start the application process.' }
        if ($Headless) {
            # Drain both streams concurrently; waiting before reading can deadlock.
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
        }
        # PowerShell's call operator need not wait for a GUI-subsystem executable.
        # Wait on the actual process and read its status, never a stale LASTEXITCODE.
        $process.WaitForExit()
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            StandardOutput = if ($Headless) { $stdout.GetAwaiter().GetResult() } else { '' }
            StandardError = if ($Headless) { $stderr.GetAwaiter().GetResult() } else { '' }
        }
    } finally { $process.Dispose() }
}

function Get-HelpdeskArguments {
    param([ValidateSet('Scan', 'Helpdesk', 'NoHistory')][string]$Mode, [string]$Root,
        [string]$ReportPath, [ValidateRange(1, 86400)][int]$TimeoutSeconds = 300)
    if ($Mode -eq 'Scan') {
        if (-not $Root -or -not $ReportPath) { throw 'Scan requires an explicit root and report path.' }
        '--scan'; $Root; '--json'; $ReportPath; '--backend'; 'normal'; '--timeout'; "$TimeoutSeconds"
    } else {
        if ($ReportPath) { throw 'GUI sessions export evidence interactively; ReportPath is only for Scan.' }
        if ($Mode -eq 'Helpdesk') { '--helpdesk' } else { '--no-history' }
        if ($Root) { $Root }
    }
}

function Assert-HelpdeskGuiContext {
    param([string]$Mode, [bool]$IsSystem, [bool]$IsInteractive, [int]$SessionId)
    if ($Mode -ne 'Scan' -and ($IsSystem -or -not $IsInteractive -or $SessionId -eq 0)) {
        throw 'GUI launch requires the intended interactive user desktop. Use Scan for SYSTEM/background tools.'
    }
}

function Assert-HelpdeskReportDestination {
    param([string]$Path)
    if ([IO.Path]::GetFileName($Path) -match '[. ]$') { throw 'Report path must not have a trailing dot or space.' }
    $fullPath = [IO.Path]::GetFullPath($Path)
    $rootLength = [IO.Path]::GetPathRoot($fullPath).Length
    if ($fullPath.Substring($rootLength).Contains(':') -or
        [IO.Path]::GetFileName($fullPath) -match '[. ]$') { throw 'Report path must name a regular file, without an alternate stream or trailing dot/space.' }
    Assert-NoReleaseReparsePath $fullPath
    if (Test-Path -LiteralPath $fullPath) { throw 'Report path already exists; choose a new incident evidence filename.' }
    if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($fullPath)) -PathType Container)) { throw 'Report parent must already exist with approved access controls.' }
    return $fullPath
}

function Assert-HelpdeskSignatureRecord {
    param($Signature, [string]$ExpectedThumbprint)
    if ($ExpectedThumbprint -notmatch '^[a-fA-F0-9]{40}$') { throw 'An independently approved SHA-1 certificate thumbprint is required.' }
    if ($Signature.Status -ne 'Valid' -or -not $Signature.SignerCertificate -or
        $Signature.SignerCertificate.Thumbprint -ine $ExpectedThumbprint -or
        -not $Signature.TimeStamperCertificate) {
        throw 'Expected a Windows-trusted Authenticode signature from the approved certificate with a timestamp.'
    }
}

function Assert-HelpdeskSignature {
    param([string]$Path, [string]$ExpectedThumbprint)
    Assert-HelpdeskSignatureRecord (Get-AuthenticodeSignature -LiteralPath $Path) $ExpectedThumbprint
}

function New-HelpdeskStage {
    param([string]$Executable, [string]$ExpectedSha256, [string]$StageDirectory,
        [ValidateSet('Scan', 'Helpdesk', 'NoHistory')][string]$Mode = 'Helpdesk', [string]$ApprovedSignerThumbprint)
    if ($ExpectedSha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'ExpectedSha256 must be an independently approved SHA-256 digest.' }
    $source = (Resolve-Path -LiteralPath $Executable -ErrorAction Stop).ProviderPath
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw 'Executable must be a file.' }
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ine $ExpectedSha256) { throw 'Executable hash mismatch; nothing staged.' }
    $stage = [IO.Path]::GetFullPath($StageDirectory)
    Assert-NoReleaseReparsePath $stage
    if (Test-Path -LiteralPath $stage) { throw 'StageDirectory must not exist; existing customer/deployment files will not be replaced.' }
    $parent = [IO.Path]::GetDirectoryName($stage)
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { throw 'Stage parent must already exist with approved access controls.' }
    # Directory.CreateDirectory allows an existing directory. New-Item without Force fails instead.
    New-Item -ItemType Directory -Path $stage -ErrorAction Stop | Out-Null
    Assert-NoReleaseReparsePath $stage
    $destination = Join-Path $stage 'DiskScope.exe'
    $inputFile = [IO.File]::Open($source, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        $outputFile = [IO.File]::Open($destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $inputFile.CopyTo($outputFile) } finally { $outputFile.Dispose() }
    } finally { $inputFile.Dispose() }
    if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ine $ExpectedSha256) { throw 'Staged executable hash mismatch; do not execute this stage.' }
    if ($ApprovedSignerThumbprint) { Assert-HelpdeskSignature $destination $ApprovedSignerThumbprint }
    $policy = if ($Mode -eq 'NoHistory') { '{"helpdesk":false,"no_history":true}' } else { '{"helpdesk":true,"no_history":true}' }
    $policyFile = [IO.File]::Open((Join-Path $stage 'diskscope-policy.json'), [IO.FileMode]::CreateNew)
    try {
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($policy)
        $policyFile.Write($bytes, 0, $bytes.Length)
    } finally { $policyFile.Dispose() }
    return $destination
}
