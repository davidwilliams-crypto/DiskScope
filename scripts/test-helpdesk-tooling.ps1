param([string]$Executable, [switch]$RequireJunction)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
. (Join-Path $PSScriptRoot 'helpdesk-tools.ps1')
$script:checks = 0
function Assert-True([bool]$Condition, [string]$Description) {
    if (-not $Condition) { throw $Description }
    $script:checks++
}
function Assert-Rejected([scriptblock]$Operation, [string]$Description) {
    $rejected = $false
    try { & $Operation | Out-Null } catch { $rejected = $true }
    Assert-True $rejected $Description
}
function Assert-Arguments([string[]]$Actual, [string[]]$Expected, [string]$Description) {
    $matches = $Actual.Count -eq $Expected.Count
    for ($index = 0; $matches -and $index -lt $Expected.Count; $index++) { $matches = $Actual[$index] -ceq $Expected[$index] }
    Assert-True $matches $Description
}
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('diskscope-helpdesk-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
try {
    # Reproduce the original native splat failure before testing the workflow's actual expression.
    $probe = 'import json,sys;print(json.dumps(sys.argv[1:]))'
    $bad = switch ('no-gui') { 'no-gui' { @('--no-default-features') } }
    $badResult = @((& python -c $probe @bad) | ConvertFrom-Json)
    if ($LASTEXITCODE -ne 0) { throw 'Python native argument probe failed.' }
    if ($PSVersionTable.PSVersion.Major -ge 7) {
        Assert-True ($badResult.Count -eq '--no-default-features'.Length -and $badResult[0] -ceq '-') 'Scalar native splat failure did not reproduce.'
    } else { Write-Output 'Windows PowerShell legacy native argument handling did not split the scalar; fixed-array cases still required.' }
    $workflow = Get-Content -LiteralPath (Join-Path $PSScriptRoot '../.github/workflows/windows.yml') -Raw
    $expression = [regex]::Match($workflow, '(?s)\$featureArgs = (?<expression>@\(switch .*?\}\))').Groups['expression'].Value
    Assert-True ([bool]$expression) 'Workflow must explicitly wrap its switch in an array.'
    foreach ($configuration in @('default', 'diagnostics', 'no-gui')) {
        $scriptText = '$featureArgs = ' + $expression.Replace('${{ matrix.features }}', $configuration) + '; & python -c $probe @featureArgs'
        $actual = @((& ([scriptblock]::Create($scriptText))) | ConvertFrom-Json | ForEach-Object { $_ })
        if ($LASTEXITCODE -ne 0) { throw "Native argument probe failed: $configuration" }
        $expected = @(switch ($configuration) { 'diagnostics' { '--features'; 'diagnostics' } 'no-gui' { '--no-default-features' } })
        Assert-Arguments $actual $expected "Workflow argument boundaries changed: $configuration"
    }
    $awkwardArguments = @('', 'a b', ('Unicode-' + [char]0x00e9), 'C:\folder with spaces\', 'C:\trailing\\', 'embedded"quote', 'slashes\\"quote')
    $nativeProbe = Invoke-HelpdeskProcess (Get-Command python -ErrorAction Stop).Source (@('-c', $probe) + $awkwardArguments) -Headless
    $roundtrip = @($nativeProbe.StandardOutput | ConvertFrom-Json | ForEach-Object { $_ })
    Assert-True ($nativeProbe.ExitCode -eq 0) 'Waited native argument probe failed.'
    Assert-Arguments $roundtrip $awkwardArguments 'Waited process argument boundaries changed.'
    $nativeFailure = Invoke-HelpdeskProcess (Get-Command python).Source @('-c', 'import sys;sys.stderr.write("expected failure");sys.exit(7)') -Headless
    Assert-True ($nativeFailure.ExitCode -eq 7 -and $nativeFailure.StandardError -ceq 'expected failure') 'Waited process lost stderr or nonzero exit status.'
    $unicodeStreams = Invoke-HelpdeskProcess (Get-Command python).Source @('-c', 'import sys;sys.stdout.buffer.write(bytes([195,169]));sys.stderr.buffer.write(bytes([195,169]))') -Headless
    Assert-True ($unicodeStreams.StandardOutput -ceq [string][char]0x00e9 -and $unicodeStreams.StandardError -ceq [string][char]0x00e9) 'Native UTF-8 diagnostic streams were decoded incorrectly.'
    foreach ($mode in @('Helpdesk', 'NoHistory')) {
        $flag = if ($mode -eq 'Helpdesk') { '--helpdesk' } else { '--no-history' }
        Assert-Arguments @(Get-HelpdeskArguments $mode) @($flag) "$mode without root"
        Assert-Arguments @(Get-HelpdeskArguments $mode 'C:\folder with spaces\') @($flag, 'C:\folder with spaces\') "$mode root must be positional"
        Assert-Rejected { Get-HelpdeskArguments $mode 'C:\' 'report.json' } 'GUI accepted CLI report destination.'
        Assert-Rejected { Assert-HelpdeskGuiContext $mode $true $true 1 } 'SYSTEM GUI accepted.'
        Assert-Rejected { Assert-HelpdeskGuiContext $mode $false $false 1 } 'Noninteractive GUI accepted.'
        Assert-Rejected { Assert-HelpdeskGuiContext $mode $false $true 0 } 'Session-zero GUI accepted.'
        Assert-HelpdeskGuiContext $mode $false $true 1
    }
    Assert-Arguments @(Get-HelpdeskArguments 'Scan' 'C:\folder with spaces' 'D:\report with spaces.json' 45) @('--scan', 'C:\folder with spaces', '--json', 'D:\report with spaces.json', '--backend', 'normal', '--timeout', '45') 'CLI argument boundaries'
    Assert-Rejected { Get-HelpdeskArguments 'Scan' '' 'report.json' } 'Missing CLI root accepted.'
    Assert-Rejected { Get-HelpdeskArguments 'Scan' 'C:\' '' } 'Missing CLI report accepted.'
    Assert-HelpdeskGuiContext 'Scan' $true $false 0
    $existingReport = Join-Path $fixture 'existing.json'
    [IO.File]::WriteAllText($existingReport, 'must remain')
    Assert-Rejected { Assert-HelpdeskReportDestination $existingReport } 'Existing report accepted.'
    Assert-Rejected { Assert-HelpdeskReportDestination ($existingReport + ':stream') } 'Alternate-stream report accepted.'
    Assert-Rejected { Assert-HelpdeskReportDestination (Join-Path $fixture 'report.json.') } 'Ambiguous trailing-dot report accepted.'
    Assert-Rejected { Assert-HelpdeskReportDestination (Join-Path $fixture 'missing-parent/report.json') } 'Missing report parent accepted.'
    Assert-True ((Assert-HelpdeskReportDestination (Join-Path $fixture 'new.json')) -ceq (Join-Path $fixture 'new.json')) 'New report destination rejected.'
    Assert-True ([IO.File]::ReadAllText($existingReport) -ceq 'must remain') 'Rejected report modified existing bytes.'

    $source = Join-Path $fixture 'source.exe'
    [IO.File]::WriteAllText($source, 'disposable bytes; never executed')
    $hash = (Get-FileHash -LiteralPath $source).Hash
    $stage = Join-Path $fixture 'stage with spaces'
    Assert-Rejected { New-HelpdeskStage $source ('0' * 64) $stage } 'Hash mismatch accepted.'
    Assert-True (-not (Test-Path -LiteralPath $stage)) 'Hash mismatch created a stage.'
    $staged = New-HelpdeskStage $source $hash $stage
    Assert-True ((Get-FileHash -LiteralPath $staged).Hash -ceq $hash) 'Staged bytes differ.'
    $policy = Get-Content -LiteralPath (Join-Path $stage 'diskscope-policy.json') -Raw | ConvertFrom-Json
    Assert-True ($policy.helpdesk -and $policy.no_history) 'Default stage policy must be private diagnostic-only.'
    Assert-Rejected { New-HelpdeskStage $source $hash $stage } 'Existing deployment overwritten.'
    Assert-True ((Get-FileHash -LiteralPath $staged).Hash -ceq $hash) 'Rejected redeployment modified existing files.'
    $privateStage = Join-Path $fixture 'private'
    New-HelpdeskStage $source $hash $privateStage 'NoHistory' | Out-Null
    $policy = Get-Content -LiteralPath (Join-Path $privateStage 'diskscope-policy.json') -Raw | ConvertFrom-Json
    Assert-True (-not $policy.helpdesk -and $policy.no_history) 'NoHistory must not claim diagnostic-only actions.'
    $target = Join-Path $fixture 'target'
    New-Item -ItemType Directory -Path $target | Out-Null
    $link = Join-Path $fixture 'redirect'
    $junctionCreated = $false
    try { New-Item -ItemType Junction -Path $link -Target $target | Out-Null; $junctionCreated = $true }
    catch {
        if ($RequireJunction) { throw }
        Write-Warning "Junction capability unavailable; reparse integration NOT tested: $($_.Exception.Message)"
    }
    if ($junctionCreated) {
        try { Assert-Rejected { New-HelpdeskStage $source $hash (Join-Path $link 'new-stage') } 'Reparse stage ancestor accepted.' }
        finally { [IO.Directory]::Delete($link) }
    }

    $thumbprint = 'A' * 40
    $signature = [pscustomobject]@{ Status = 'Valid'; SignerCertificate = [pscustomobject]@{ Thumbprint = $thumbprint }; TimeStamperCertificate = [pscustomobject]@{} }
    Assert-HelpdeskSignatureRecord $signature $thumbprint
    $script:checks++
    Assert-Rejected { Assert-HelpdeskSignatureRecord $signature ('B' * 40) } 'Wrong publisher accepted.'
    Assert-Rejected { Assert-HelpdeskSignatureRecord $signature 'not-a-thumbprint' } 'Missing approved publisher accepted.'
    $signature.Status = 'NotTrusted'
    Assert-Rejected { Assert-HelpdeskSignatureRecord $signature $thumbprint } 'Untrusted signature accepted.'
    $signature.Status = 'Valid'; $signature.TimeStamperCertificate = $null
    Assert-Rejected { Assert-HelpdeskSignatureRecord $signature $thumbprint } 'Untimestamped signature accepted.'
    Assert-Rejected { Assert-HelpdeskSignature $source $thumbprint } 'Unsigned fixture accepted as an approved publisher.'

    if ($Executable) {
        # Opt-in actual candidate integration: disposable scan, private fresh stage, no file actions.
        $Executable = (Resolve-Path -LiteralPath $Executable).ProviderPath
        $root = Join-Path $fixture ('scan with spaces ' + [char]0x00e9)
        New-Item -ItemType Directory -Path $root | Out-Null
        [IO.File]::WriteAllText((Join-Path $root 'known.txt'), 'known bytes')
        $report = Join-Path $fixture 'report with spaces.json'
        $hostExecutable = (Get-Process -Id $PID).Path
        $launch = @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'invoke-helpdesk.ps1'), '-Executable', $Executable,
            '-ExpectedSha256', (Get-FileHash -LiteralPath $Executable).Hash, '-StageDirectory', (Join-Path $fixture 'candidate'), '-Mode', 'Scan', '-Root', $root, '-ReportPath', $report)
        & $hostExecutable @launch
        Assert-True ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $report)) 'Candidate wrapper scan failed.'
        $result = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
        Assert-True ($result.complete -and $result.files -eq 1 -and $result.logical_bytes -eq 11 -and $result.errors_total -eq 0) 'Candidate report differs from the disposable known-answer inventory.'
        Assert-True ($result.evidence.schema_version -eq 1 -and $result.evidence.context.session -match '(?i)CLI' -and $result.evidence.context.session -match '(?i)no automatic profile persistence' -and $result.evidence.context.session -match '(?i)helpdesk') 'Candidate is missing the hardened evidence/session-policy envelope.'
        $before = (Get-FileHash -LiteralPath $report).Hash
        & $hostExecutable @launch
        Assert-True ($LASTEXITCODE -eq 20) 'Existing evidence path must fail preflight with 20.'
        Assert-True ((Get-FileHash -LiteralPath $report).Hash -ceq $before) 'Existing evidence modified.'
        $failedLaunch = $launch.Clone()
        $failedLaunch[[Array]::IndexOf($failedLaunch, '-Root') + 1] = Join-Path $fixture 'missing-scan-root'
        $failedLaunch[[Array]::IndexOf($failedLaunch, '-StageDirectory') + 1] = Join-Path $fixture 'failed-candidate'
        $failedLaunch[[Array]::IndexOf($failedLaunch, '-ReportPath') + 1] = Join-Path $fixture 'failed-report.json'
        & $hostExecutable @failedLaunch
        Assert-True ($LASTEXITCODE -eq 1) "Application failure exit must be forwarded as 1 without being called a launch failure; received $LASTEXITCODE."
        $invalidStage = Join-Path $fixture 'invalid-policy-candidate'
        $invalidExecutable = New-HelpdeskStage $Executable (Get-FileHash -LiteralPath $Executable).Hash $invalidStage
        [IO.File]::WriteAllText((Join-Path $invalidStage 'diskscope-policy.json'), '{"helpdesk":true,"unexpected":true}')
        $invalidReport = Join-Path $fixture 'must-not-exist.json'
        $invalidResult = Invoke-HelpdeskProcess $invalidExecutable @('--scan', $root, '--json', $invalidReport) -Headless
        Assert-True ($invalidResult.ExitCode -eq 2 -and -not (Test-Path -LiteralPath $invalidReport)) 'Malformed/unknown companion policy must fail closed before scanning/report creation.'
    }
    Write-Output "Helpdesk tooling: $script:checks checks passed. Candidate integration=$([bool]$Executable)."
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notlike 'diskscope-helpdesk-*') { throw 'Refusing cleanup outside owned helpdesk fixture.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
# Several assertions deliberately launch native failures. Do not leak the last
# expected nonzero status to a caller or GitHub's PowerShell step epilogue.
exit 0
