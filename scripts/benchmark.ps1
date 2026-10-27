param(
    [Parameter(Mandatory=$true)][string]$ScanRoot,
    [string]$DiskScope,
    [string]$WinDirStat = (Join-Path (Split-Path -Parent $PSScriptRoot) 'research\artifacts\WinDirStat-2.9.3-x64.exe'),
    [string]$OutputDirectory = (Join-Path (Split-Path -Parent $PSScriptRoot) 'research\benchmark'),
    [ValidateRange(5,100)][int]$Runs = 5
)
$ErrorActionPreference='Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'package-tools.ps1')
$diskScopeIdentity = Get-DiskScopeBenchmarkTarget -ProjectRoot $projectRoot -Override $DiskScope
$winDirStatIdentity = Get-BenchmarkExecutableIdentity -Path $WinDirStat
$DiskScope = $diskScopeIdentity.path
$WinDirStat = $winDirStatIdentity.path
function ConvertTo-WindowsArgument {
    param([AllowEmptyString()][string]$Argument)
    # Windows doubles backslashes before a quote and before the closing quote.
    $escaped = [regex]::Replace($Argument, '(\\*)"', '$1$1\"')
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    '"' + $escaped + '"'
}
foreach ($path in @($ScanRoot,$DiskScope,$WinDirStat)) {if (-not (Test-Path -LiteralPath $path)) {throw "Missing path: $path"}}
$ScanRoot = (Resolve-Path -LiteralPath $ScanRoot).ProviderPath
if ($ScanRoot.Contains('"') -or $OutputDirectory.Contains('"')) {throw 'Unsupported quote in argument path'}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).ProviderPath
$results=@()
$referenceTotals=$null
$observedDiskScopeVersion=$null
for ($i=0;$i -lt $Runs;$i++) {
    # Alternate order to reduce a consistent cache-order advantage. No caches are flushed.
    $order=if ($i%2 -eq 0) {@('DiskScope','WinDirStat')} else {@('WinDirStat','DiskScope')}
    foreach ($appName in $order) {
        $report=Join-Path $OutputDirectory "$appName-$i.json"
        if (Test-Path -LiteralPath $report) {throw "Report exists; choose a fresh output directory: $report"}
        if ($appName -eq 'DiskScope') {
            $exe=$DiskScope
            $identity=$diskScopeIdentity
            $argumentValues=@('--scan',$ScanRoot,'--json',$report,'--backend','normal')
        } else {
            $exe=$WinDirStat
            $identity=$winDirStatIdentity
            $argumentValues=@('/SaveTo',$report,$ScanRoot)
        }
        $arguments=($argumentValues | ForEach-Object {ConvertTo-WindowsArgument $_}) -join ' '
        $watch=[Diagnostics.Stopwatch]::StartNew()
        $process=Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru
        if (-not $process.WaitForExit(120000)) {throw "$appName did not finish within two minutes; process $($process.Id) is left running for inspection"}
        $watch.Stop()
        if ($process.ExitCode -ne 0) {throw "$appName exited with code $($process.ExitCode)"}
        if (-not (Test-Path -LiteralPath $report)) {throw "$appName did not produce a report"}
        $data=Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
        if ($appName -eq 'DiskScope') {
            $expectedVersion = if ($diskScopeIdentity.expected_version) { $diskScopeIdentity.expected_version } else { $observedDiskScopeVersion }
            $diskScopeIdentity.version = Get-DiskScopeReportVersion -Report $data -ExpectedVersion $expectedVersion
            $observedDiskScopeVersion = $diskScopeIdentity.version
            $diskScopeIdentity.version_source = 'scan report'
            if (-not $data.complete -or $data.errors_total -ne 0) {throw 'DiskScope reported incomplete results; timings are not comparable'}
            $totals=[pscustomobject]@{files=[long]$data.files;folders=[long]$data.folders;logical_bytes=[long]$data.logical_bytes}
        } else {
            $rootRecord=@($data)[0]
            if ($null -eq $rootRecord.'Logical Size' -or $null -eq $rootRecord.Files -or $null -eq $rootRecord.Folders) {throw 'Unrecognized WinDirStat JSON root format'}
            $totals=[pscustomobject]@{files=[long]$rootRecord.Files;folders=[long]$rootRecord.Folders;logical_bytes=[long]$rootRecord.'Logical Size'}
        }
        if ($null -eq $referenceTotals) {$referenceTotals=$totals}
        elseif ($totals.files -ne $referenceTotals.files -or $totals.folders -ne $referenceTotals.folders -or $totals.logical_bytes -ne $referenceTotals.logical_bytes) {
            throw "Result counts/bytes differ for $appName run $i; do not interpret these timings as a matched scan"
        }
        $row=[pscustomobject]@{application=$appName;executable_path=$identity.path;executable_version=$identity.version;executable_sha256=$identity.sha256;run=$i;wall_ms=$watch.Elapsed.TotalMilliseconds;files=$totals.files;folders=$totals.folders;logical_bytes=$totals.logical_bytes;report=$report}
        $results+=$row
        $row | ConvertTo-Json -Compress
    }
}
$summary=[pscustomobject]@{
    date=(Get-Date).ToUniversalTime().ToString('o');root=$ScanRoot;runs=$Runs;timings=$results;verified_totals=$referenceTotals
    executables=[pscustomobject]@{DiskScope=$diskScopeIdentity;WinDirStat=$winDirStatIdentity}
    conditions='Warm/OS-managed cache, alternating order; no cache flush; normal folder scanner. WinDirStat uses saved settings. Verify result totals before interpretation.'
    limitation='End-to-end report commands include startup and output. WinDirStat writes its full tree; DiskScope writes a summary and largest100 files, so this is not an isolated scan-engine comparison.'
}
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'timings.json') -Encoding utf8
