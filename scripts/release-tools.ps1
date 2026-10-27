# Shared release packaging helpers. Compatible with Windows PowerShell 5.1 and 7.
function Assert-NoReleaseReparsePath {
    param([string]$Path)
    $candidate = [IO.Path]::GetFullPath($Path)
    while ($candidate) {
        if (Test-Path -LiteralPath $candidate) {
            $item = Get-Item -LiteralPath $candidate -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Release paths must not contain reparse points: $candidate" }
        }
        $candidate = [IO.Path]::GetDirectoryName($candidate.TrimEnd('\', '/'))
    }
}

function Get-ReleaseVersion {
    param([string]$ProjectRoot)
    $manifest = Get-Content -LiteralPath (Join-Path $ProjectRoot 'Cargo.toml') -Raw
    $package = [regex]::Match($manifest, '(?ms)^\s*\[package\]\s*\r?\n(?<body>.*?)(?=^\s*\[|\z)')
    $version = [regex]::Match($package.Groups['body'].Value, '(?m)^\s*version\s*=\s*"(?<v>[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)"\s*(?:#.*)?$')
    if (-not $version.Success) { throw 'Cargo.toml must contain an explicit package version.' }
    return $version.Groups['v'].Value
}

function Get-ReleaseFiles {
    param([string]$Directory)
    $root = Get-Item -LiteralPath $Directory -Force -ErrorAction Stop
    if (-not $root.PSIsContainer) { throw 'The release payload must be a directory.' }
    $pending = [Collections.Generic.Stack[IO.DirectoryInfo]]::new()
    $pending.Push($root)
    while ($pending.Count -gt 0) {
        $directoryItem = $pending.Pop()
        if ($directoryItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Reparse points are not allowed in release payloads: $($directoryItem.FullName)"
        }
        foreach ($entry in Get-ChildItem -LiteralPath $directoryItem.FullName -Force) {
            if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Reparse points are not allowed in release payloads: $($entry.FullName)"
            }
            if ($entry.PSIsContainer) { $pending.Push($entry) } else { $entry }
        }
    }
}

function Assert-ReleasePayload {
    param([string]$Directory, [string]$Version)
    Assert-NoReleaseReparsePath $Directory
    $root = (Resolve-Path -LiteralPath $Directory -ErrorAction Stop).ProviderPath.TrimEnd('\', '/')
    $files = @(Get-ReleaseFiles -Directory $root | Sort-Object FullName)
    $prefix = $root + [IO.Path]::DirectorySeparatorChar
    $actual = @{}
    foreach ($file in $files) {
        if (-not $file.FullName.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Payload path escaped its root.' }
        $relative = $file.FullName.Substring($prefix.Length).Replace('\', '/')
        $actual.Add($relative, $file)
    }
    foreach ($required in @('DiskScope.exe', 'README.txt', 'VALIDATION.md', 'PERFORMANCE.md', 'THIRD_PARTY_NOTICES.md', 'SHA256SUMS.txt')) {
        if (-not $actual.ContainsKey($required)) { throw "Required payload file missing: $required" }
    }
    if (-not @($actual.Keys | Where-Object { $_ -like 'licenses/*' }).Count) { throw 'Dependency license files are missing.' }
    $readme = Get-Content -LiteralPath $actual['README.txt'].FullName -TotalCount 1
    if ($readme -notmatch ('^DiskScope ' + [regex]::Escape($Version) + '(?:\s|$)')) { throw 'Payload README version does not match Cargo.toml.' }
    $seen = @{}
    foreach ($line in Get-Content -LiteralPath $actual['SHA256SUMS.txt'].FullName) {
        if ($line -notmatch '^([a-fA-F0-9]{64})  (.+)$') { throw 'Malformed payload checksum entry.' }
        $hash = $Matches[1]
        $relative = $Matches[2]
        if ($relative -match '[\\:\r\n]' -or $relative.StartsWith('/') -or
            @($relative.Split('/') | Where-Object { $_ -in @('', '.', '..') }).Count -or
            $relative -ieq 'SHA256SUMS.txt' -or $seen.ContainsKey($relative)) { throw "Unsafe or duplicate checksum path: $relative" }
        if (-not $actual.ContainsKey($relative)) { throw "Manifest file missing: $relative" }
        if ((Get-FileHash -LiteralPath $actual[$relative].FullName -Algorithm SHA256).Hash -ine $hash) { throw "Payload checksum mismatch: $relative" }
        $seen.Add($relative, $true)
    }
    if ($seen.Count -ne $actual.Count - 1) { throw 'Payload contains files not covered by its checksum manifest.' }
    return $files
}

function Assert-SeparateReleaseDirectories {
    param([string]$Payload, [string]$Output)
    Assert-NoReleaseReparsePath $Payload
    Assert-NoReleaseReparsePath $Output
    $a = [IO.Path]::GetFullPath($Payload).TrimEnd('\', '/') + '\'
    $b = [IO.Path]::GetFullPath($Output).TrimEnd('\', '/') + '\'
    if ($a.StartsWith($b, [StringComparison]::OrdinalIgnoreCase) -or $b.StartsWith($a, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Payload and release output directories must not overlap.'
    }
}

function ConvertTo-NsisLiteral {
    param([string]$Value)
    if ($Value -match '[\r\n\x00]') { throw 'NSIS literals cannot contain control characters.' }
    return $Value.Replace('$', '$$').Replace('"', '$\"')
}

function Write-ReleasePayloadInclude {
    param([string]$Directory, [string]$Destination)
    $root = (Resolve-Path -LiteralPath $Directory).ProviderPath.TrimEnd('\', '/')
    $prefix = $root + '\'
    $files = @(Get-ReleaseFiles -Directory $root | Sort-Object FullName)
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('!macro InstallPayload')
    $directories = @{}
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($prefix.Length)
        $parent = [IO.Path]::GetDirectoryName($relative)
        $out = '$INSTDIR'
        if ($parent) { $out += '\' + (ConvertTo-NsisLiteral $parent) }
        $lines.Add('  SetOutPath "' + $out + '"')
        $lines.Add('  File "' + (ConvertTo-NsisLiteral $file.FullName) + '"')
        $lines.Add('  IfErrors payload_copy_failed')
        while ($parent) {
            $directories[$parent] = $true
            $parent = [IO.Path]::GetDirectoryName($parent)
        }
    }
    $lines.Add('!macroend')
    $lines.Add('!macro RemovePayload')
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($prefix.Length)
        $lines.Add('  Delete "$INSTDIR\' + (ConvertTo-NsisLiteral $relative) + '"')
        $lines.Add('  IfFileExists "$INSTDIR\' + (ConvertTo-NsisLiteral $relative) + '" un.PayloadRemoveFailed')
    }
    foreach ($directory in @($directories.Keys | Sort-Object { $_.Length } -Descending)) {
        $lines.Add('  RMDir "$INSTDIR\' + (ConvertTo-NsisLiteral $directory) + '"')
    }
    $lines.Add('!macroend')
    [IO.File]::WriteAllLines($Destination, $lines, [Text.UTF8Encoding]::new($true))
}

function New-PortableArchive {
    param([string]$Directory, [string]$Destination, [string]$Version)
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $root = (Resolve-Path -LiteralPath $Directory).ProviderPath.TrimEnd('\', '/')
    $prefix = $root + '\'
    $stream = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew)
    try {
        $zip = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create, $true)
        try {
            foreach ($file in @(Get-ReleaseFiles -Directory $root | Sort-Object FullName)) {
                $relative = $file.FullName.Substring($prefix.Length).Replace('\', '/')
                $entry = $zip.CreateEntry("DiskScope-$Version-portable/$relative", [IO.Compression.CompressionLevel]::Optimal)
                $entry.LastWriteTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
                $inputStream = [IO.File]::OpenRead($file.FullName)
                try {
                    $entryStream = $entry.Open()
                    try { $inputStream.CopyTo($entryStream) } finally { $entryStream.Dispose() }
                } finally { $inputStream.Dispose() }
            }
        } finally { $zip.Dispose() }
    } finally { $stream.Dispose() }
}
