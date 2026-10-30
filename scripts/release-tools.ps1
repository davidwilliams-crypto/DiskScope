# Shared release packaging helpers. Compatible with Windows PowerShell 5.1 and 7.
function Get-ReleaseCargoArtifact {
    param([string[]]$Messages, [string]$PackageId, [string]$ManifestPath)
    $artifacts = @()
    $finished = $false
    foreach ($line in $Messages) {
        $message = $line | ConvertFrom-Json -ErrorAction Stop
        if ($message.reason -eq 'build-finished') {
            if (-not $message.success) { throw 'Cargo reported an unsuccessful build.' }
            $finished = $true
        }
        if ($message.reason -eq 'compiler-artifact' -and $message.package_id -ceq $PackageId -and
            $message.target.name -ceq 'diskscope' -and $message.target.kind -contains 'bin' -and
            -not $message.profile.test -and $message.executable) {
            if ([IO.Path]::GetFullPath($message.manifest_path) -ine [IO.Path]::GetFullPath($ManifestPath)) { throw 'Cargo artifact has an unexpected manifest.' }
            $artifacts += $message
        }
    }
    if (-not $finished -or $artifacts.Count -ne 1) { throw "Expected exactly one DiskScope executable from a completed Cargo build; found $($artifacts.Count)." }
    $artifact = $artifacts[0]
    if (-not [IO.Path]::IsPathRooted($artifact.executable) -or [IO.Path]::GetExtension($artifact.executable) -ine '.exe' -or
        -not (Test-Path -LiteralPath $artifact.executable -PathType Leaf)) { throw 'Cargo-reported Windows executable is missing or invalid.' }
    return $artifact
}

function Copy-ReleaseExecutable {
    param($Artifact, [string]$Destination)
    # Verify the copied bytes against the reported artifact, never a guessed target path.
    $hash = (Get-FileHash -LiteralPath $Artifact.executable -Algorithm SHA256).Hash
    Copy-Item -LiteralPath $Artifact.executable -Destination $Destination -Force
    if ((Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -cne $hash) { throw 'Packaged executable differs from the Cargo artifact.' }
    return $hash.ToLowerInvariant()
}

function ConvertTo-RustReleaseVersion {
    param([string]$Version)
    # Cargo permits both 1.92 and 1.92.0; .NET treats an omitted patch as -1.
    if ($Version -match '^\d+\.\d+$') { $Version += '.0' }
    return [version]$Version
}

function Get-LockedRustFloor {
    param($Metadata)
    $active = @{}
    foreach ($node in $Metadata.resolve.nodes) { $active[$node.id] = $true }
    $packages = @($Metadata.packages | Where-Object { $active.ContainsKey($_.id) -and $_.rust_version })
    if (-not $packages.Count) { throw 'Resolved dependency compiler requirements are missing.' }
    $highest = $packages | Sort-Object { ConvertTo-RustReleaseVersion $_.rust_version } -Descending | Select-Object -First 1
    return [ordered]@{
        minimum = $highest.rust_version
        packages = @($packages | Where-Object { (ConvertTo-RustReleaseVersion $_.rust_version) -eq (ConvertTo-RustReleaseVersion $highest.rust_version) } | ForEach-Object { "$($_.name)@$($_.version)" } | Sort-Object)
    }
}

function Get-ReleaseSourceIdentity {
    param([string]$ProjectRoot)
    $revision = & git -C $ProjectRoot rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $revision -notmatch '^[a-f0-9]{40}$') { throw 'Release provenance requires a Git checkout with a revision.' }
    $paths = & git -C $ProjectRoot -c core.quotepath=false ls-files --cached --others --exclude-standard
    if ($LASTEXITCODE -ne 0) { throw 'Source inventory failed.' }
    $files = @($paths | Sort-Object -Unique | ForEach-Object {
        if ($_ -match '[\r\n]' -or $_.StartsWith('"')) { throw 'Source inventory contains an unsupported filename.' }
        $file = Join-Path $ProjectRoot $_
        $hash = if (Test-Path -LiteralPath $file -PathType Leaf) { (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() } else { $null }
        [ordered]@{ path = $_; sha256 = $hash }
    })
    $status = @(& git -C $ProjectRoot status --porcelain=v1 --untracked-files=normal)
    if ($LASTEXITCODE -ne 0) { throw 'Source status inspection failed.' }
    $identity = [ordered]@{ revision = $revision; dirty = ($status.Count -gt 0); files = $files }
    $bytes = [Text.Encoding]::UTF8.GetBytes(($identity | ConvertTo-Json -Depth 10 -Compress))
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try { $identity.fingerprint = [BitConverter]::ToString($algorithm.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant() }
    finally { $algorithm.Dispose() }
    return $identity
}

function Write-ReleaseProvenance {
    param([string]$Destination, $Source, $Artifact, $Build, [string]$Executable)
    $path = Join-Path $Destination 'BUILD_PROVENANCE.json'
    $record = [ordered]@{
        schema_version = 1; built_at_utc = [DateTime]::UtcNow.ToString('o')
        source = $Source; build = $Build
        artifact = [ordered]@{ package_id = $Artifact.package_id; target = $Artifact.target; profile = $Artifact.profile; features = @($Artifact.features); fresh = $Artifact.fresh }
        executable = [ordered]@{ name = [IO.Path]::GetFileName($Executable); sha256 = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    [IO.File]::WriteAllText($path, ($record | ConvertTo-Json -Depth 20), [Text.UTF8Encoding]::new($false))
    return $path
}

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

# The first line of the portable README names the release ("DiskScope 0.3.0 ...").
# build.ps1 checks scripts\portable-readme.txt with this before compiling, and
# Assert-ReleasePayload checks the README.txt copied into the payload, so the
# early check cannot accept a file the final check would reject.
function Assert-ReleaseReadmeVersion {
    param([string]$Path, [string]$Version)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "README is missing: $Path" }
    $firstLine = Get-Content -LiteralPath $Path -TotalCount 1
    if ($firstLine -notmatch ('^DiskScope ' + [regex]::Escape($Version) + '(?:\s|$)')) {
        throw "README must start with 'DiskScope $Version' (the Cargo.toml version): $Path"
    }
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
    foreach ($required in @('DiskScope.exe', 'README.txt', 'LICENSE', 'VALIDATION.md', 'PERFORMANCE.md', 'THIRD_PARTY_NOTICES.md', 'SHA256SUMS.txt', 'HELP_DESK_RUNBOOK.md', 'HELP_DESK_QUALIFICATION.md',
            'helpdesk-tools/invoke-helpdesk.ps1', 'helpdesk-tools/helpdesk-tools.ps1', 'helpdesk-tools/release-tools.ps1', 'helpdesk-tools/verify-release-signatures.ps1')) {
        if (-not $actual.ContainsKey($required)) { throw "Required payload file missing: $required" }
    }
    if (-not @($actual.Keys | Where-Object { $_ -like 'licenses/*' }).Count) { throw 'Dependency license files are missing.' }
    Assert-ReleaseReadmeVersion -Path $actual['README.txt'].FullName -Version $Version
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
