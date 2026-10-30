<# Explicit allowlist: fixtures only. Never run every ignored test on a customer endpoint. #>
param([switch]$IncludeRecycle)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$tests = @(
    'permanent_delete_removes_only_the_confirmed_fixture',
    'permanent_delete_refuses_a_changed_confirmed_file',
    'permanent_delete_refuses_a_replaced_path',
    'permanent_delete_refuses_an_open_writer',
    'permanent_delete_does_not_clear_the_read_only_attribute',
    'junction_ancestor_is_refused_at_preparation',
    'junction_added_after_confirmation_is_refused_at_commit'
)
if ($IncludeRecycle) {
    # Qualify separately on an interactive standard-user fixed NTFS lab volume.
    # Elevated hosts can skip paths internally and cannot prove recycle success.
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    try { $principal = [Security.Principal.WindowsPrincipal]::new($identity)
        if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or
            $identity.User.Value -eq 'S-1-5-18' -or -not [Environment]::UserInteractive) {
            throw 'Recycle qualification requires a standard-user interactive lab session.'
        }
    } finally { $identity.Dispose() }
    $tests += @('recycle_sends_only_the_confirmed_fixture_to_the_bin',
        'prepared_plan_carries_a_recycle_verdict_that_does_not_block_the_fixture',
        'sink_veto_leaves_the_file_intact', 'recycle_with_an_owner_window_completes')
}
Push-Location $projectRoot
try {
    foreach ($test in $tests) {
        # Confirm the test exists; Cargo otherwise treats a misspelled filter as green.
        $name = 'file_actions::tests::' + $test
        $listed = @(& cargo test --locked --lib $name -- --ignored --list)
        if ($LASTEXITCODE -ne 0 -or -not ($listed -match ([regex]::Escape($name) + ': test$'))) { throw "Missing fixture: $name" }
        & cargo test --locked --lib $name -- --ignored --exact --test-threads=1 --nocapture
        if ($LASTEXITCODE -ne 0) { throw "Native fixture failed: $name" }
    }
    Write-Output "Native fixture gate passed: $($tests.Count) explicitly selected tests. Recycle enabled=$IncludeRecycle."
} finally { Pop-Location }
