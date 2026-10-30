# Import is read-only. Fakes exercise these mechanics only, never product claims.
. (Join-Path $PSScriptRoot 'helpdesk-tools.ps1')

function Get-QualificationGates {
    @(
        @('candidate-integrity', 'release', 'automated'),
        @('known-answer-cli', 'accounting', 'automated'),
        @('failed-root-exit', 'safety', 'automated'),
        @('report-no-clobber', 'privacy', 'automated'),
        @('resource-observation', 'resources', 'automated'),
        @('fixture-preservation', 'safety', 'automated'),
        @('release-validation', 'release', 'manual'),
        @('token-uac', 'safety', 'manual'),
        @('persistence-privacy', 'privacy', 'manual'),
        @('native-actions', 'safety', 'manual'),
        @('devices-accounting', 'accounting', 'manual'),
        @('resource-classes', 'resources', 'manual'),
        @('history-evidence', 'evidence', 'manual'),
        @('screenconnect', 'deployment', 'manual'),
        @('keyboard-narrator', 'accessibility', 'manual'),
        @('publisher-edr', 'publisher', 'manual'),
        @('installer-rollback', 'deployment', 'manual'),
        @('technician-pilot', 'usability', 'manual')
    ) | ForEach-Object { [pscustomobject]@{ id = $_[0]; area = $_[1]; kind = $_[2] } }
}

function Get-QualificationDecision {
    param([object[]]$Checks)
    $gates = @(Get-QualificationGates)
    $seen = @{}
    foreach ($check in $Checks) {
        if ($check.id -notin @($gates.id)) { throw "Unknown qualification check: $($check.id)" }
        if ($seen.ContainsKey($check.id)) { throw "Duplicate qualification check: $($check.id)" }
        if ($check.status -notin @('passed', 'failed', 'pending')) { throw "Invalid qualification status: $($check.status)" }
        $seen[$check.id] = $check.status
    }
    $areaRows = @()
    foreach ($area in @($gates.area | Sort-Object -Unique)) {
        $ids = @($gates | Where-Object { $_.area -eq $area } | ForEach-Object { $_.id })
        $failed = @($ids | Where-Object { $seen[$_] -eq 'failed' })
        $pending = @($ids | Where-Object { -not $seen.ContainsKey($_) -or $seen[$_] -eq 'pending' })
        $status = if ($failed.Count) { 'failed' } elseif ($pending.Count) { 'pending' } else { 'passed' }
        $areaRows += [pscustomobject]@{
            area = $area; status = $status; eligible_for_90_or_A = $status -eq 'passed'
            failed_gate_ids = $failed; pending_gate_ids = $pending
            numerical_readiness = $null
        }
    }
    $overall = if (@($areaRows | Where-Object { $_.status -eq 'failed' }).Count) { 'failed' }
        elseif (@($areaRows | Where-Object { $_.status -eq 'pending' }).Count) { 'pending' } else { 'passed' }
    [pscustomobject]@{
        status = $overall; eligible_for_90_or_A = $overall -eq 'passed'; areas = $areaRows
        numerical_readiness = $null; measured_population_reliability = $null
        rule = 'Every required gate must pass for eligibility. Missing evidence is pending. Eligibility does not assign a numerical readiness score or reliability estimate.'
    }
}

function Invoke-QualificationProcess {
    param([string]$Executable, [string[]]$Arguments, [int]$TimeoutSeconds = 60)
    try { Invoke-HelpdeskProcess $Executable $Arguments -Headless -TimeoutSeconds $TimeoutSeconds -ObserveResources }
    catch {
        [pscustomobject]@{
            ExitCode = $null; StandardOutput = ''; StandardError = ''; StartError = $_.Exception.Message
            TimedOut = $false; ElapsedMilliseconds = $null
            Resource = [pscustomobject]@{ sample_count = 0; observed_peak_private_bytes = $null; observed_peak_working_set_bytes = $null; observation_error = $_.Exception.Message }
        }
    }
}

function Write-QualificationNewFile {
    param([string]$Path, [AllowEmptyString()][string]$Content)
    Assert-NoReleaseReparsePath ([IO.Path]::GetFullPath($Path))
    $file = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($Content)
        $file.Write($bytes, 0, $bytes.Length)
    } finally { $file.Dispose() }
}
