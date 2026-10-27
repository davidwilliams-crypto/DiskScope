# Shared package operations, usable from Windows PowerShell 5.1 and PowerShell 7.
function Get-DiskScopeReportVersion {
    param($Report, [string]$ExpectedVersion)
    if ($Report.application -cne 'DiskScope' -or $Report.version -isnot [string] -or
        [string]::IsNullOrWhiteSpace($Report.version)) {
        throw 'The benchmark report must identify DiskScope and its version.'
    }
    if ($ExpectedVersion -and $Report.version -cne $ExpectedVersion) {
        throw "DiskScope report version '$($Report.version)' does not match expected version '$ExpectedVersion'. Rebuild the current package or explicitly select a historical executable with -DiskScope."
    }
    return $Report.version
}

function Get-BenchmarkExecutableIdentity {
    param([string]$Path)
    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) { throw "Executable is not a file: $resolved" }
    $resourceVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($resolved).FileVersion
    if ([string]::IsNullOrWhiteSpace($resourceVersion)) { $resourceVersion = $null }
    [pscustomobject]@{
        path = $resolved
        sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
        file_version = $resourceVersion
        version = $resourceVersion
        version_source = $(if ($resourceVersion) { 'PE file version' } else { $null })
    }
}

function Get-DiskScopeBenchmarkTarget {
    param([string]$ProjectRoot, [string]$Override)
    $version = $null
    if ($Override) {
        $selected = $Override
    } else {
        $manifest = Get-Content -LiteralPath (Join-Path $ProjectRoot 'Cargo.toml') -Raw
        $package = [regex]::Match($manifest, '(?ms)^\s*\[package\]\s*\r?\n(?<body>.*?)(?=^\s*\[|\z)')
        $match = [regex]::Match($package.Groups['body'].Value, '(?m)^\s*version\s*=\s*"(?<version>[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)"\s*(?:#.*)?$')
        if (-not $match.Success) { throw 'Cargo.toml must contain an explicit package version for the benchmark default.' }
        $version = $match.Groups['version'].Value
        $selected = Join-Path $ProjectRoot ("dist\$version\DiskScope.exe")
    }
    $identity = Get-BenchmarkExecutableIdentity -Path $selected
    $identity | Add-Member -NotePropertyName expected_version -NotePropertyValue $version
    return $identity
}

function Get-PackageChecksums {
    param([string]$Directory)
    $root = (Resolve-Path -LiteralPath $Directory -ErrorAction Stop).ProviderPath
    if (-not (Test-Path -LiteralPath $root -PathType Container)) { throw "Package is not a directory: $root" }
    # Resolve dot segments and trailing separators before deriving relative paths.
    # Path.GetRelativePath is unavailable in Windows PowerShell 5.1's .NET Framework.
    $prefix = $root.TrimEnd([char[]]@('\', '/')) + [IO.Path]::DirectorySeparatorChar
    $manifest = Join-Path $root 'SHA256SUMS.txt'
    Get-ChildItem -LiteralPath $root -File -Recurse |
        Where-Object { $_.FullName -ne $manifest } |
        Sort-Object FullName |
        ForEach-Object {
            if (-not $_.FullName.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Package file is outside the normalized package directory: $($_.FullName)"
            }
            $relative = $_.FullName.Substring($prefix.Length).Replace('\', '/')
            $digest = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            "$digest  $relative"
        }
}
