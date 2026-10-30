param([string]$Executable)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'qualification-tools.ps1')
$script:checks = 0
function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }; $script:checks++
}
function Assert-Rejected([scriptblock]$Operation, [string]$Message) {
    $rejected = $false
    try { & $Operation | Out-Null } catch { $rejected = $true }
    Assert-True $rejected $Message
}
$required = @(Get-QualificationGates)
$fakePassing = @($required | ForEach-Object { [pscustomobject]@{ id = $_.id; status = 'passed' } })
Assert-True ((Get-QualificationDecision $fakePassing).status -eq 'passed') 'Complete synthetic matrix should exercise the decision happy path.'
Assert-True ((Get-QualificationDecision @()).status -eq 'pending') 'Absent checks must remain pending.'
$withoutLast = @($fakePassing | Select-Object -SkipLast 1)
Assert-True (-not (Get-QualificationDecision $withoutLast).eligible_for_90_or_A) 'Missing manual gate was treated as A eligible.'
$pending = @($fakePassing | ForEach-Object { [pscustomobject]@{ id = $_.id; status = $_.status } })
$pending[-1].status = 'pending'
Assert-True ((Get-QualificationDecision $pending).status -eq 'pending') 'Unavailable manual evidence passed by averaging.'
$failed = @($fakePassing | ForEach-Object { [pscustomobject]@{ id = $_.id; status = $_.status } })
$failed[0].status = 'failed'
Assert-True ((Get-QualificationDecision $failed).status -eq 'failed') 'A failed safety check was hidden by successful checks.'
Assert-True (-not (Get-QualificationDecision $failed).eligible_for_90_or_A) 'A failed safety check became A eligible.'
Assert-Rejected { Get-QualificationDecision @($fakePassing + $fakePassing[0]) } 'Duplicate check IDs accepted.'
Assert-Rejected { Get-QualificationDecision @([pscustomobject]@{ id = 'invented'; status = 'passed' }) } 'Unknown check ID accepted.'
Assert-Rejected { Get-QualificationDecision @([pscustomobject]@{ id = $required[0].id; status = 'skipped' }) } 'Skip accepted as a status.'

# Real child process, fake application semantics. This checks the harness only;
# it is never written as evidence that DiskScope itself passed qualification.
$python = (Get-Command python -ErrorAction Stop).Source
$probe = Invoke-QualificationProcess $python @('-c', 'import sys,time; print("stream output");sys.stderr.write("expected failure");time.sleep(.2);sys.exit(7)') 5
Assert-True ($probe.ExitCode -eq 7 -and $probe.StandardError -eq 'expected failure' -and $probe.StandardOutput.Trim() -eq 'stream output') 'Waited child output/exit contract failed.'
Assert-True ($probe.Resource.sample_count -gt 0 -and $probe.Resource.observed_peak_private_bytes -gt 0) 'Actual child resource observation absent.'
$timeout = Invoke-QualificationProcess $python @('-c', 'import time;time.sleep(10)') 1
Assert-True ($timeout.TimedOut -and $timeout.ExitCode -ne 0) 'Timeout became a successful process.'
$unavailable = Invoke-QualificationProcess 'Z:\unavailable\candidate.exe' @() 1
Assert-True ($null -eq $unavailable.ExitCode -and $unavailable.StartError) 'Unavailable executable did not produce explicit start failure.'
Assert-True ($null -eq $unavailable.Resource.observed_peak_private_bytes) 'Unavailable executable fabricated memory data.'

$output = Join-Path ([IO.Path]::GetTempPath()) ('diskscope-qualification-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $output | Out-Null
$canary = Join-Path $output 'canary.json'
Write-QualificationNewFile $canary 'preserve bytes'
Assert-Rejected { Write-QualificationNewFile $canary 'replace' } 'Evidence overwrite accepted.'
Assert-True ([IO.File]::ReadAllText($canary) -ceq 'preserve bytes') 'Existing evidence changed.'
$hostExe = (Get-Process -Id $PID).Path
$runner = Join-Path $PSScriptRoot 'qualify-helpdesk.ps1'
$unavailableDirectory = Join-Path $output 'unavailable-candidate'
$unavailableRun = Invoke-QualificationProcess $hostExe @('-NoProfile', '-File', $runner, '-Executable', (Join-Path $output 'absent.exe'),
    '-ExpectedSha256', ('0' * 64), '-OutputDirectory', $unavailableDirectory) 30
Assert-True ($unavailableRun.ExitCode -eq 1) 'Unavailable candidate did not fail qualification.'
$unavailableReport = Get-Content -LiteralPath (Join-Path $unavailableDirectory 'qualification.json') -Raw | ConvertFrom-Json
Assert-True ($unavailableReport.decision.status -eq 'failed' -and -not $unavailableReport.decision.eligible_for_90_or_A) 'Unavailable qualification became passing/A eligible.'
$fakeExe = Join-Path $output 'unexecutable.exe'
Write-QualificationNewFile $fakeExe 'Not a Windows executable. Harness negative test only.'
$fakeDirectory = Join-Path $output 'fake-candidate'
$fakeRun = Invoke-QualificationProcess $hostExe @('-NoProfile', '-File', $runner, '-Executable', $fakeExe,
    '-ExpectedSha256', (Get-FileHash -LiteralPath $fakeExe).Hash, '-OutputDirectory', $fakeDirectory) 30
Assert-True ($fakeRun.ExitCode -eq 1) 'Unexecutable fake candidate passed qualification.'
$fakeReport = Get-Content -LiteralPath (Join-Path $fakeDirectory 'qualification.json') -Raw | ConvertFrom-Json
Assert-True ($fakeReport.automated_status -eq 'failed' -and -not $fakeReport.decision.eligible_for_90_or_A) 'Failed child probe became passing/A eligible.'
$mismatchDirectory = Join-Path $output 'hash-mismatch'
$mismatch = Invoke-QualificationProcess $hostExe @('-NoProfile', '-File', $runner, '-Executable', $fakeExe,
    '-ExpectedSha256', ('0' * 64), '-OutputDirectory', $mismatchDirectory) 30
Assert-True ($mismatch.ExitCode -eq 1 -and -not (Test-Path -LiteralPath (Join-Path $mismatchDirectory 'candidate'))) 'Hash mismatch executed/staged candidate or passed.'
if ($Executable) {
    $candidate = (Resolve-Path -LiteralPath $Executable).ProviderPath
    $runDirectory = Join-Path $output 'actual-candidate'
    $result = Invoke-QualificationProcess $hostExe @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'qualify-helpdesk.ps1'), '-Executable', $candidate,
        '-ExpectedSha256', (Get-FileHash -LiteralPath $candidate).Hash, '-OutputDirectory', $runDirectory) 120
    Assert-True ($result.ExitCode -eq 2) "Actual automated checks should exit 2 for outstanding manual gates: $($result.StandardError)"
    $report = Get-Content -LiteralPath (Join-Path $runDirectory 'qualification.json') -Raw | ConvertFrom-Json
    Assert-True ($report.automated_status -eq 'passed' -and $report.decision.status -eq 'pending' -and -not $report.decision.eligible_for_90_or_A) 'Actual candidate status is not passed automated / pending qualification.'
    Assert-True (@($report.checks | Where-Object { $_.status -eq 'failed' }).Count -eq 0) 'Actual candidate safety check failed.'
    Assert-True ($report.candidate.sha256 -ieq (Get-FileHash -LiteralPath $candidate).Hash) 'Candidate hash not bound to report.'
    $before = (Get-FileHash -LiteralPath (Join-Path $runDirectory 'qualification.json')).Hash
    $rerun = Invoke-QualificationProcess $hostExe @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'qualify-helpdesk.ps1'), '-Executable', $candidate,
        '-ExpectedSha256', (Get-FileHash -LiteralPath $candidate).Hash, '-OutputDirectory', $runDirectory) 120
    Assert-True ($rerun.ExitCode -eq 20 -and (Get-FileHash -LiteralPath (Join-Path $runDirectory 'qualification.json')).Hash -ceq $before) 'Runner reused or overwrote an evidence directory.'
}
Write-Output "Qualification harness: $script:checks checks passed. Actual candidate integration=$([bool]$Executable). Synthetic decision tests are not product qualification. Evidence retained: $output"
exit 0
