<#
Optional Authenticode release signing. Only the opted-in release workflow runs
this (repository variable AUTHENTICODE_SIGNING = 'true'); the unsigned path
never invokes it. The certificate arrives through the environment, never the
command line: AUTHENTICODE_CERT_PFX_BASE64 and AUTHENTICODE_CERT_PASSWORD are
required, AUTHENTICODE_TIMESTAMP_URL and AUTHENTICODE_SIGNER_THUMBPRINT are
optional promotions of the same defaults scripts/verify-release-signatures.ps1
enforces.

After signing, the SHA-256 manifest beside the signed files is regenerated so
Assert-ReleasePayload keeps describing the shipped bytes. BUILD_PROVENANCE.json
keeps recording the pre-signing executable digest: signing is a post-build
step, and the signed payload digest remains covered by the refreshed manifest.
#>
param(
    [Parameter(Mandatory)][string[]]$Path,
    [Parameter(Mandatory)][string]$ManifestDirectory,
    [string]$SignTool
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
. (Join-Path $PSScriptRoot 'release-tools.ps1')

if (-not $env:AUTHENTICODE_CERT_PFX_BASE64 -or -not $env:AUTHENTICODE_CERT_PASSWORD) {
    throw 'Authenticode signing was requested but the certificate environment is absent.'
}
if (-not $SignTool) {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($command) { $SignTool = $command.Source }
    else {
        $kitsBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
        $candidates = @(Get-ChildItem -LiteralPath $kitsBin -Recurse -Filter signtool.exe -File -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending)
        if (-not $candidates.Count) { throw "signtool.exe was not found on PATH or under $kitsBin." }
        $SignTool = $candidates[0].FullName
    }
}
$SignTool = (Resolve-Path -LiteralPath $SignTool -ErrorAction Stop).ProviderPath

$manifestRoot = (Resolve-Path -LiteralPath $ManifestDirectory -ErrorAction Stop).ProviderPath.TrimEnd('\', '/')
$manifestPrefix = $manifestRoot + [IO.Path]::DirectorySeparatorChar
$resolvedPaths = @()
foreach ($file in $Path) {
    $resolved = (Resolve-Path -LiteralPath $file -ErrorAction Stop).ProviderPath
    if (-not $resolved.StartsWith($manifestPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Signed file must live under the manifest directory: $resolved"
    }
    $resolvedPaths += $resolved
}

$timestamp = if ($env:AUTHENTICODE_TIMESTAMP_URL) { $env:AUTHENTICODE_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
$certificate = Join-Path ([IO.Path]::GetTempPath()) ('diskscope-signing-' + [guid]::NewGuid().ToString('N') + '.pfx')
try {
    [IO.File]::WriteAllBytes($certificate, [Convert]::FromBase64String($env:AUTHENTICODE_CERT_PFX_BASE64))
    foreach ($resolved in $resolvedPaths) {
        & $SignTool sign /fd SHA256 /tr $timestamp /td SHA256 /f $certificate /p $env:AUTHENTICODE_CERT_PASSWORD $resolved
        if ($LASTEXITCODE -ne 0) { throw "signtool failed for $resolved (exit $LASTEXITCODE)." }
        & $SignTool verify /pa /all /tw $resolved
        if ($LASTEXITCODE -ne 0) { throw "Signature verification failed for $resolved (exit $LASTEXITCODE)." }
        Write-Output "Signed and verified: $resolved"
    }
} finally {
    if (Test-Path -LiteralPath $certificate) { Remove-Item -LiteralPath $certificate -Force }
}

[IO.File]::WriteAllLines((Join-Path $manifestRoot 'SHA256SUMS.txt'),
    [string[]]@(Get-PackageChecksums -Directory $manifestRoot), [Text.UTF8Encoding]::new($false))

if ($env:AUTHENTICODE_SIGNER_THUMBPRINT) {
    & (Join-Path $PSScriptRoot 'verify-release-signatures.ps1') -Path $resolvedPaths -ApprovedSignerThumbprint $env:AUTHENTICODE_SIGNER_THUMBPRINT -SignTool $SignTool
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
