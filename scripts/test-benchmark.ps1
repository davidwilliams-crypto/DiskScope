# Exercises the real benchmark script using tiny owned report-producing executables.
# These fixtures do not scan any user files or launch either application.
$ErrorActionPreference = 'Stop'
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) { throw 'This Windows integration test requires the .NET Framework C# compiler.' }
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('diskscope-benchmark-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
try {
    $scripts = Join-Path $fixture 'scripts'
    $package = Join-Path $fixture 'dist\9.8.7'
    $scan = Join-Path $fixture 'empty scan'
    foreach ($directory in @($scripts, $package, $scan)) { New-Item -ItemType Directory -Path $directory -Force | Out-Null }
    foreach ($name in @('benchmark.ps1', 'package-tools.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $scripts $name)
    }
    '[package]', 'name = "fixture"', 'version = "9.8.7"' | Set-Content -LiteralPath (Join-Path $fixture 'Cargo.toml') -Encoding utf8
    $source = Join-Path $fixture 'report-fixture.cs'
    @'
using System;
using System.IO;
using System.Reflection;
[assembly: AssemblyFileVersion("1.2.3.4")]
class ReportFixture {
    static int Main(string[] args) {
        if (args.Length == 3 && args[0] == "/SaveTo") {
            File.WriteAllText(args[1], "[{\"Files\":0,\"Folders\":0,\"Logical Size\":0}]");
            return 0;
        }
        if (args.Length != 6 || args[0] != "--scan" || args[2] != "--json" || args[4] != "--backend" || args[5] != "normal") return 2;
        string version = File.ReadAllText(Assembly.GetExecutingAssembly().Location + ".version").Trim();
        File.WriteAllText(args[3], "{\"application\":\"DiskScope\",\"version\":\"" + version + "\",\"complete\":true,\"errors_total\":0,\"files\":0,\"folders\":0,\"logical_bytes\":0}");
        return 0;
    }
}
'@ | Set-Content -LiteralPath $source -Encoding utf8
    $executable = Join-Path $package 'DiskScope.exe'
    & $compiler /nologo /target:exe "/out:$executable" $source
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile the owned report fixture.' }
    [IO.File]::WriteAllText($executable + '.version', '9.8.7')
    $benchmark = Join-Path $scripts 'benchmark.ps1'
    $current = Join-Path $fixture 'current results'
    & $benchmark -ScanRoot $scan -WinDirStat $executable -OutputDirectory $current -Runs 5 | Out-Null
    $summary = Get-Content -LiteralPath (Join-Path $current 'timings.json') -Raw | ConvertFrom-Json
    $digest = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($summary.timings.Count -ne 10 -or $summary.executables.DiskScope.path -cne $executable -or
        $summary.executables.DiskScope.sha256 -cne $digest -or $summary.executables.DiskScope.version -cne '9.8.7' -or
        $summary.executables.DiskScope.version_source -cne 'scan report' -or $summary.executables.WinDirStat.version -cne '1.2.3.4') {
        throw 'Benchmark summary did not retain the executable identity and report version.'
    }
    foreach ($row in $summary.timings) {
        if ($row.executable_path -cne $executable -or $row.executable_sha256 -cne $digest -or -not $row.executable_version) {
            throw 'A timing row is missing executable provenance.'
        }
    }
    [IO.File]::WriteAllText($executable + '.version', '0.2.0')
    $rejected = $false
    try { & $benchmark -ScanRoot $scan -WinDirStat $executable -OutputDirectory (Join-Path $fixture 'stale') -Runs 5 | Out-Null }
    catch {
        if ($_.Exception.Message -notlike "*report version '0.2.0' does not match expected version '9.8.7'*") { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw 'Stale executable report was accepted as the current package.' }
    $historical = Join-Path $fixture 'historical'
    & $benchmark -ScanRoot $scan -DiskScope $executable -WinDirStat $executable -OutputDirectory $historical -Runs 5 | Out-Null
    $summary = Get-Content -LiteralPath (Join-Path $historical 'timings.json') -Raw | ConvertFrom-Json
    if ($summary.executables.DiskScope.version -cne '0.2.0' -or $summary.executables.DiskScope.expected_version) {
        throw 'Explicit historical selection did not preserve its own report version.'
    }
    Write-Output 'Benchmark integration regressions passed.'
} finally {
    $resolvedFixture = [IO.Path]::GetFullPath($fixture)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolvedFixture.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolvedFixture) -notlike 'diskscope-benchmark-test-*') {
        throw 'Refusing cleanup outside the owned benchmark fixture.'
    }
    Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
}
