param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [Parameter(Mandatory = $true)][ValidatePattern('^[a-fA-F0-9]{64}$')][string]$ExpectedSha256,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string[]]$PackageArtifact = @(),
    [ValidateRange(1, 300)][int]$ProcessTimeoutSeconds = 60
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'qualification-tools.ps1')
$started = [DateTime]::UtcNow.ToString('o')
$script:rows = [Collections.Generic.List[object]]::new()
function Add-Check([string]$Id, [string]$Status, $Expected, $Actual, [string]$Reason) {
    $script:rows.Add([pscustomobject]@{ id = $Id; status = $Status; observed_at_utc = [DateTime]::UtcNow.ToString('o'); expected = $Expected; actual = $Actual; reason = $Reason })
}
function Get-Status([bool]$Passed) { if ($Passed) { 'passed' } else { 'failed' } }
function Get-FixtureSnapshot([string]$Root) {
    @(Get-ChildItem -LiteralPath $Root -File -Recurse | Sort-Object FullName | ForEach-Object {
        $_.FullName.Substring($Root.Length) + '|' + $_.Length + '|' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    })
}
try {
    $output = [IO.Path]::GetFullPath($OutputDirectory)
    Assert-NoReleaseReparsePath $output
    if (Test-Path -LiteralPath $output) { throw 'OutputDirectory must be new; evidence directories are never reused or overwritten.' }
    if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($output)) -PathType Container)) { throw 'OutputDirectory parent must already exist.' }
    New-Item -ItemType Directory -Path $output | Out-Null
} catch { Write-Error -ErrorAction Continue "Qualification preflight: $($_.Exception.Message)"; exit 20 }

$candidate = [ordered]@{ path = [IO.Path]::GetFullPath($Executable); expected_sha256 = $ExpectedSha256.ToLowerInvariant(); sha256 = $null; package_artifacts = @(); provenance = $null }
$runs = [ordered]@{}
$fatal = $null
try {
    Assert-NoReleaseReparsePath $candidate.path
    $candidate.sha256 = (Get-FileHash -LiteralPath $candidate.path -Algorithm SHA256).Hash.ToLowerInvariant()
    $matches = $candidate.sha256 -ceq $candidate.expected_sha256
    Add-Check 'candidate-integrity' (Get-Status $matches) $candidate.expected_sha256 $candidate.sha256 'Source executable must match the operator-supplied digest before it is staged or executed.'
    if (-not $matches) { throw 'Candidate hash mismatch; candidate not executed.' }
    foreach ($artifact in $PackageArtifact) {
        $artifactPath = (Resolve-Path -LiteralPath $artifact).ProviderPath
        Assert-NoReleaseReparsePath $artifactPath
        $candidate.package_artifacts += [pscustomobject]@{ path = $artifactPath; sha256 = (Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $provenancePath = Join-Path ([IO.Path]::GetDirectoryName($candidate.path)) 'BUILD_PROVENANCE.json'
    if (Test-Path -LiteralPath $provenancePath -PathType Leaf) {
        $provenanceText = [IO.File]::ReadAllText($provenancePath)
        $provenance = $provenanceText | ConvertFrom-Json
        if ($provenance.executable.sha256 -ine $candidate.sha256) { throw 'Sibling build provenance refers to different executable bytes.' }
        $candidate.provenance = [pscustomobject]@{ sha256 = (Get-FileHash -LiteralPath $provenancePath).Hash.ToLowerInvariant(); source = $provenance.source; built_at_utc = $provenance.built_at_utc }
        Write-QualificationNewFile (Join-Path $output 'BUILD_PROVENANCE.json') $provenanceText
    }
    $fixture = Join-Path $output ('fixture with spaces ' + [char]0x00e9)
    New-Item -ItemType Directory -Path (Join-Path $fixture 'nested/empty') -Force | Out-Null
    Write-QualificationNewFile (Join-Path $fixture 'known.txt') 'known bytes'
    Write-QualificationNewFile (Join-Path $fixture 'nested/data.bin') ('x' * 4096)
    Write-QualificationNewFile (Join-Path $fixture 'empty.bin') ''
    $snapshot = @(Get-FixtureSnapshot $fixture)
    $staged = New-HelpdeskStage $candidate.path $ExpectedSha256 (Join-Path $output 'candidate') 'Helpdesk'
    $reportPath = Join-Path $output 'scan.json'
    $scanArgs = @(Get-HelpdeskArguments 'Scan' $fixture $reportPath 30)
    $runs.scan = Invoke-QualificationProcess $staged $scanArgs $ProcessTimeoutSeconds
    $scan = if (Test-Path -LiteralPath $reportPath -PathType Leaf) { Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json } else { $null }
    $known = $runs.scan.ExitCode -eq 0 -and -not $runs.scan.TimedOut -and $null -ne $scan -and $scan.complete -and $scan.files -eq 3 -and $scan.folders -eq 2 -and $scan.logical_bytes -eq 4107 -and $scan.errors_total -eq 0 -and $scan.evidence.schema_version -eq 1 -and $scan.evidence.context.session -match '(?i)helpdesk'
    Add-Check 'known-answer-cli' (Get-Status $known) @{ exit = 0; complete = $true; files = 3; child_folders = 2; logical_bytes = 4107; errors = 0; evidence_schema = 1; helpdesk = $true } @{ process = $runs.scan; report = $scan } 'Actual candidate, disposable normal-backend inventory with spaces and Unicode. This does not qualify whole-drive/provider accounting.'

    if (Test-Path -LiteralPath $reportPath -PathType Leaf) {
        $before = (Get-FileHash -LiteralPath $reportPath).Hash
        $runs.no_clobber = Invoke-QualificationProcess $staged $scanArgs $ProcessTimeoutSeconds
        $after = (Get-FileHash -LiteralPath $reportPath).Hash
        Add-Check 'report-no-clobber' (Get-Status ($runs.no_clobber.ExitCode -eq 1 -and -not $runs.no_clobber.TimedOut -and $before -ceq $after)) @{ exit = 1; unchanged = $true } @{ process = $runs.no_clobber; before_sha256 = $before; after_sha256 = $after } 'Direct CLI must refuse an existing report without changing its bytes; no retry hides a failure.'
    } else { Add-Check 'report-no-clobber' 'pending' 'existing report refused unchanged' $null 'Initial scan did not create a report; prerequisite unavailable.' }
    $missing = Join-Path $output 'intentionally absent root'
    $failureReportPath = Join-Path $output 'failed-root.json'
    $runs.failed_root = Invoke-QualificationProcess $staged @(Get-HelpdeskArguments 'Scan' $missing $failureReportPath 30) $ProcessTimeoutSeconds
    $failureReport = if (Test-Path -LiteralPath $failureReportPath -PathType Leaf) { Get-Content -LiteralPath $failureReportPath -Raw | ConvertFrom-Json } else { $null }
    $honestFailure = $runs.failed_root.ExitCode -eq 1 -and -not $runs.failed_root.TimedOut -and $null -ne $failureReport -and $failureReport.complete -eq $false -and $failureReport.state -eq 'Scan failed' -and $failureReport.errors_total -gt 0
    Add-Check 'failed-root-exit' (Get-Status $honestFailure) @{ exit = 1; complete = $false; state = 'Scan failed'; errors_minimum = 1 } @{ process = $runs.failed_root; report = $failureReport } 'An unavailable root must return failure with honest incomplete/error evidence.'
    $resource = $runs.scan.Resource
    Add-Check 'resource-observation' $(if ($resource.sample_count -gt 0 -and $resource.observed_peak_private_bytes -gt 0) { 'passed' } else { 'pending' }) 'At least one positive actual process sample' $resource 'Sampled only this bounded fixture; no low-RAM, responsiveness, large-inventory, first-useful-result or reliability qualification follows.'
    $afterSnapshot = @(Get-FixtureSnapshot $fixture)
    $unchanged = @(Compare-Object $snapshot $afterSnapshot).Count -eq 0
    Add-Check 'fixture-preservation' (Get-Status $unchanged) $snapshot $afterSnapshot 'Read-only CLI cases must preserve every owned fixture file hash and length.'
    if ((Get-FileHash -LiteralPath $candidate.path).Hash -ine $ExpectedSha256 -or (Get-FileHash -LiteralPath $staged).Hash -ine $ExpectedSha256) { throw 'Candidate executable changed during qualification.' }
} catch {
    $fatal = $_.Exception.Message
    # Even a failed setup/probe is retained as an explicit automated failure.
    if (@($script:rows | Where-Object { $_.id -eq 'candidate-integrity' }).Count -eq 0) { Add-Check 'candidate-integrity' 'failed' $ExpectedSha256 $candidate.sha256 $fatal }
    elseif (@($script:rows | Where-Object { $_.id -eq 'candidate-integrity' })[0].status -eq 'passed') {
        $row = @($script:rows | Where-Object { $_.id -eq 'candidate-integrity' })[0]
        $row.status = 'failed'; $row.reason = $fatal
    }
}
foreach ($gate in @(Get-QualificationGates)) {
    if (@($script:rows | Where-Object { $_.id -eq $gate.id }).Count -eq 0) {
        Add-Check $gate.id 'pending' 'Exact-candidate evidence and named reviewer for all supported scenarios' $null $(if ($gate.kind -eq 'manual') { 'Not exercised by this runner. Complete the qualification manual template; no certificate or lab is assumed.' } else { 'Automated observation unavailable; inspect fatal_error and process records.' })
    }
}
$autoRows = @($script:rows | Where-Object { $_.id -in @((Get-QualificationGates | Where-Object { $_.kind -eq 'automated' }).id) })
$automated = if (@($autoRows | Where-Object { $_.status -eq 'failed' }).Count) { 'failed' } elseif (@($autoRows | Where-Object { $_.status -eq 'pending' }).Count) { 'pending' } else { 'passed' }
$decision = Get-QualificationDecision @($script:rows)
$document = [ordered]@{
    schema_version = 1; run_id = [IO.Path]::GetFileName($output); started_at_utc = $started; finished_at_utc = [DateTime]::UtcNow.ToString('o')
    candidate = $candidate; host = @{ os_version = [Environment]::OSVersion.VersionString; powershell = $PSVersionTable.PSVersion.ToString(); os_64bit = [Environment]::Is64BitOperatingSystem; process_64bit = [Environment]::Is64BitProcess }
    fixture = @{ files = 3; logical_bytes = 4107; description = 'New isolated ordinary files only; no customer files or native file actions.' }
    automated_status = $automated; decision = $decision; fatal_error = $fatal; checks = @($script:rows); process_observations = $runs
    limitations = @('All manual gates remain pending; this output does not authorize deployment or assign numerical readiness.', 'Only the exact executable SHA-256 above was executed. Package artifact hashes are binding records, not installer tests.', 'Raw reports/paths can identify this test host. Review access and redact before sharing.', 'No automatic retries. Fixtures and raw evidence retained; no existing installation or trust store touched.')
}
Write-QualificationNewFile (Join-Path $output 'qualification.json') ($document | ConvertTo-Json -Depth 50)
Write-Output "Automated: $automated. Qualification: $($decision.status). A eligible: $($decision.eligible_for_90_or_A). Evidence: $(Join-Path $output 'qualification.json')"
# 0 = fully passed, 1 = a failed gate, 2 = pending qualification, 20 = preflight.
if ($decision.status -eq 'failed') { exit 1 }
if ($decision.status -eq 'pending') { exit 2 }
exit 0
