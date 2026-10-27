param([ValidateSet('All', 'Benchmark', 'Checksums', 'Metadata', 'Versions')][string]$Case = 'All')
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'package-tools.ps1')

function Assert-Equal($Actual, $Expected, [string]$Description) {
    if ($Actual -cne $Expected) { throw "$Description. Expected '$Expected'; got '$Actual'." }
}
function Assert-Rejected([scriptblock]$Operation, [string]$Description) {
    $rejected = $false
    try { & $Operation | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "$Description was accepted." }
}

$fixture = Join-Path ([IO.Path]::GetTempPath()) ('diskscope-tooling-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
try {
    '[package]', 'name = "fixture"', 'version = "9.8.7"', '', '[dependencies]', 'other = "1.2.3"' |
        Set-Content -LiteralPath (Join-Path $fixture 'Cargo.toml') -Encoding utf8
    $package = Join-Path $fixture 'dist\9.8.7'
    New-Item -ItemType Directory -Path $package -Force | Out-Null
    $executable = Join-Path $package 'DiskScope.exe'
    [IO.File]::WriteAllText($executable, 'abc') # Hashed only; never executed.

    if ($Case -in @('All', 'Benchmark')) {
        $target = Get-DiskScopeBenchmarkTarget -ProjectRoot $fixture
        Assert-Equal $target.path $executable 'Default benchmark must select the manifest version'
        Assert-Equal $target.expected_version '9.8.7' 'Default benchmark must require the manifest version'
        $historical = Get-DiskScopeBenchmarkTarget -ProjectRoot $fixture -Override $executable
        Assert-Equal $historical.path $executable 'Explicit benchmark path must be retained'
        Assert-Equal $historical.expected_version $null 'Explicit historical versions must remain supported'
    }

    if ($Case -in @('All', 'Metadata')) {
        $target = Get-DiskScopeBenchmarkTarget -ProjectRoot $fixture
        Assert-Equal $target.sha256 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad' 'Benchmark identity must record the selected executable hash'
    }
    if ($Case -in @('All', 'Versions')) {
        $report = [pscustomobject]@{ application = 'DiskScope'; version = '0.2.0' }
        Assert-Rejected { Get-DiskScopeReportVersion -Report $report -ExpectedVersion '9.8.7' } 'Stale default benchmark version'
        Assert-Equal (Get-DiskScopeReportVersion -Report $report) '0.2.0' 'Explicit historical report version must be recorded'
        $report.version = '9.8.7'
        Assert-Equal (Get-DiskScopeReportVersion -Report $report -ExpectedVersion '9.8.7') '9.8.7' 'Matching default report version'
        $report.version = $null
        Assert-Rejected { Get-DiskScopeReportVersion -Report $report } 'Missing report version'
        $report.version = '9.8.7'
        $report.application = 'Something else'
        Assert-Rejected { Get-DiskScopeReportVersion -Report $report } 'Non-DiskScope report'
    }
    if ($Case -in @('All', 'Checksums')) {
        $nested = Join-Path $package 'licenses'
        New-Item -ItemType Directory -Path $nested | Out-Null
        [IO.File]::WriteAllText((Join-Path $nested 'NOTICE.txt'), 'notice')
        [IO.File]::WriteAllText((Join-Path $package 'SHA256SUMS.txt'), 'old checksums')
        $awkward = Join-Path $fixture 'dist\9.8.7\..\9.8.7\'
        $lines = @(Get-PackageChecksums -Directory $awkward)
        Assert-Equal $lines.Count 2 'Only the root checksum manifest must be omitted'
        foreach ($relative in @('DiskScope.exe', 'licenses/NOTICE.txt')) {
            $file = Join-Path $package $relative
            $expected = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $relative
            if ($lines -cnotcontains $expected) { throw "Checksum must preserve the complete normalized relative path: $relative" }
        }
    }
    Write-Output 'Tooling regressions passed.'
} finally {
    $resolvedFixture = [IO.Path]::GetFullPath($fixture)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolvedFixture.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolvedFixture) -notlike 'diskscope-tooling-*') {
        throw 'Refusing cleanup outside the owned tooling fixture.'
    }
    Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
}
