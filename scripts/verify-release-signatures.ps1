<# Read-only optional promotion gate. Never signs or installs certificates. #>
param([Parameter(Mandatory)][string[]]$Path, [Parameter(Mandatory)][string]$ApprovedSignerThumbprint,
    [Parameter(Mandatory)][string]$SignTool)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'helpdesk-tools.ps1')
$SignTool = (Resolve-Path -LiteralPath $SignTool -ErrorAction Stop).ProviderPath
foreach ($file in $Path) {
    $resolvedFile = (Resolve-Path -LiteralPath $file -ErrorAction Stop).ProviderPath
    Assert-HelpdeskSignature $resolvedFile $ApprovedSignerThumbprint
    & $SignTool verify /pa /all /tw /v $resolvedFile
    if ($LASTEXITCODE -ne 0) { throw "SignTool verification failed: $resolvedFile (code $LASTEXITCODE)." }
    Write-Output "Trusted approved signer and timestamp verified: $resolvedFile"
}
