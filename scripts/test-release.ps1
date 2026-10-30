$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-tools.ps1')
. (Join-Path $PSScriptRoot 'package-tools.ps1')
$projectRoot = Split-Path -Parent $PSScriptRoot
$fixture = Join-Path $projectRoot ('target\release-tests\' + [guid]::NewGuid().ToString('N'))
$payload = Join-Path $fixture 'payload with spaces'
New-Item -ItemType Directory -Path (Join-Path $payload 'licenses\example-1.0') -Force | Out-Null
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
function Write-FixtureChecksums {
    [IO.File]::WriteAllLines((Join-Path $payload 'SHA256SUMS.txt'), [string[]]@(Get-PackageChecksums $payload), [Text.UTF8Encoding]::new($false))
}
try {
    [IO.File]::WriteAllText((Join-Path $fixture 'Cargo.toml'), "[package]`nname = `"fixture`"`nversion = `"1.2.3`"`n[dependencies]`nother = `"9.9.9`"")
    Assert-True ((Get-ReleaseVersion $fixture) -ceq '1.2.3') 'Version must come from package section.'
    New-Item -ItemType Directory -Path (Join-Path $payload 'helpdesk-tools') | Out-Null
    foreach ($name in @('DiskScope.exe', 'LICENSE', 'VALIDATION.md', 'PERFORMANCE.md', 'THIRD_PARTY_NOTICES.md', 'HELP_DESK_RUNBOOK.md', 'HELP_DESK_QUALIFICATION.md',
            'helpdesk-tools\invoke-helpdesk.ps1', 'helpdesk-tools\helpdesk-tools.ps1', 'helpdesk-tools\release-tools.ps1', 'helpdesk-tools\verify-release-signatures.ps1', 'licenses\example-1.0\NOTICE $quote.txt')) {
        [IO.File]::WriteAllText((Join-Path $payload $name), $name)
    }
    [IO.File]::WriteAllText((Join-Path $payload 'README.txt'), 'DiskScope 1.2.3 preview - Windows x64')
    Write-FixtureChecksums
    Assert-True (@(Assert-ReleasePayload $payload '1.2.3').Count -eq 14) 'Valid payload rejected.'
    $licenseFile = Join-Path $payload 'LICENSE'
    Remove-Item -LiteralPath $licenseFile
    Assert-Rejected { Assert-ReleasePayload $payload '1.2.3' } 'Missing project license accepted.'
    [IO.File]::WriteAllText($licenseFile, 'LICENSE')
    Assert-Rejected { Assert-ReleasePayload $payload '9.9.9' } 'Version mismatch accepted.'
    # build.ps1 runs this same check on the source README before compiling.
    $payloadReadme = Join-Path $payload 'README.txt'
    Assert-ReleaseReadmeVersion $payloadReadme '1.2.3'
    $script:checks++
    Assert-Rejected { Assert-ReleaseReadmeVersion $payloadReadme '1.2' } 'README version prefix accepted.'
    Assert-Rejected { Assert-ReleaseReadmeVersion $payloadReadme '1.2.34' } 'README accepted for a longer version.'
    $longerVersion = Join-Path $fixture 'longer-version-README.txt'
    [IO.File]::WriteAllText($longerVersion, 'DiskScope 1.2.30 preview - Windows x64')
    Assert-Rejected { Assert-ReleaseReadmeVersion $longerVersion '1.2.3' } 'README naming a longer version accepted.'
    $laterLine = Join-Path $fixture 'later-line-README.txt'
    [IO.File]::WriteAllText($laterLine, "Release notes`nDiskScope 1.2.3 preview - Windows x64")
    Assert-Rejected { Assert-ReleaseReadmeVersion $laterLine '1.2.3' } 'README version outside the first line accepted.'
    Assert-Rejected { Assert-ReleaseReadmeVersion (Join-Path $fixture 'absent-README.txt') '1.2.3' } 'Missing README accepted.'
    # The shipped README must name the shipped version; a version bump that
    # forgets scripts\portable-readme.txt fails here instead of after a release build.
    Assert-ReleaseReadmeVersion (Join-Path $PSScriptRoot 'portable-readme.txt') (Get-ReleaseVersion $projectRoot)
    $script:checks++
    $manifest = Join-Path $payload 'SHA256SUMS.txt'
    $original = [IO.File]::ReadAllText($manifest)
    $exe = Join-Path $payload 'DiskScope.exe'
    [IO.File]::AppendAllText($exe, 'tampered')
    Assert-Rejected { Assert-ReleasePayload $payload '1.2.3' } 'Checksum mismatch accepted.'
    [IO.File]::WriteAllText($exe, 'DiskScope.exe')
    foreach ($path in @('../outside.txt', '/rooted.txt', 'C:/rooted.txt', 'licenses/../README.txt', 'licenses\example.txt', 'README.txt:stream')) {
        [IO.File]::WriteAllText($manifest, ('0' * 64) + "  $path`n")
        Assert-Rejected { Assert-ReleasePayload $payload '1.2.3' } "Unsafe checksum path accepted: $path"
    }
    [IO.File]::WriteAllText($manifest, $original + $original)
    Assert-Rejected { Assert-ReleasePayload $payload '1.2.3' } 'Duplicate checksum entry accepted.'
    [IO.File]::WriteAllText($manifest, $original)
    $extra = Join-Path $payload 'untracked.txt'
    [IO.File]::WriteAllText($extra, 'unexpected')
    [IO.File]::SetAttributes($extra, [IO.FileAttributes]::Hidden)
    Assert-Rejected { Assert-ReleasePayload $payload '1.2.3' } 'Unmanifested hidden file accepted.'
    Remove-Item -LiteralPath $extra -Force
    $outside = Join-Path $fixture 'outside'
    New-Item -ItemType Directory -Path $outside | Out-Null
    $junction = Join-Path $payload 'redirect'
    New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null
    Assert-Rejected { Assert-ReleasePayload $payload '1.2.3' } 'Payload junction accepted.'
    # Remove only the link, never recursively traverse its target.
    [IO.Directory]::Delete($junction)
    $outputAlias = Join-Path $fixture 'output-alias'
    New-Item -ItemType Junction -Path $outputAlias -Target $payload | Out-Null
    try {
        Assert-Rejected { Assert-SeparateReleaseDirectories $payload $outputAlias } 'Output alias into the payload accepted.'
        Assert-Rejected { Assert-SeparateReleaseDirectories $payload (Join-Path $outputAlias 'new-release') } 'Output with reparse ancestor accepted.'
        Assert-Rejected { Assert-ReleasePayload $outputAlias '1.2.3' } 'Payload alias accepted.'
    } finally { [IO.Directory]::Delete($outputAlias) }
    foreach ($output in @($payload, (Join-Path $payload 'release'), $fixture, (Join-Path $payload '..\payload with spaces'))) {
        Assert-Rejected { Assert-SeparateReleaseDirectories $payload $output } 'Overlapping release directory accepted.'
    }
    Assert-SeparateReleaseDirectories $payload ($payload + '-release')
    $script:checks++
    Assert-True ((ConvertTo-NsisLiteral '$INSTDIR "quoted"') -ceq '$$INSTDIR $\"quoted$\"') 'NSIS dollar/quote escaping failed.'
    Assert-Rejected { ConvertTo-NsisLiteral "first`nsecond" } 'NSIS newline injection accepted.'
    $include = Join-Path $fixture 'payload.nsh'
    Write-ReleasePayloadInclude $payload $include
    $source = [IO.File]::ReadAllText($include)
    Assert-True ($source.Contains('NOTICE $$quote.txt')) 'Payload filenames must not interpolate NSIS variables.'
    Assert-True (-not $source.Contains('RMDir /r')) 'Uninstall must not recursively delete user files.'
    Assert-True (($source -split 'IfFileExists').Count -eq 15) 'Every installed file requires checked removal.'
    $archiveA = Join-Path $fixture 'portable-a.zip'
    $archiveB = Join-Path $fixture 'portable-b.zip'
    New-PortableArchive $payload $archiveA '1.2.3'
    New-PortableArchive $payload $archiveB '1.2.3'
    Assert-True ((Get-FileHash $archiveA).Hash -ceq (Get-FileHash $archiveB).Hash) 'Identical payload ZIPs should be reproducible.'
    $extracted = Join-Path $fixture 'extracted'
    [IO.Compression.ZipFile]::ExtractToDirectory($archiveA, $extracted)
    Assert-True (@(Assert-ReleasePayload (Join-Path $extracted 'DiskScope-1.2.3-portable') '1.2.3').Count -eq 14) 'Extracted payload differs.'
    Write-Output "Release tooling: $script:checks checks passed."
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $ownedRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot 'target\release-tests')) + '\'
    if (-not $resolved.StartsWith($ownedRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup outside release-tests.' }
    if (Test-Path -LiteralPath (Join-Path $payload 'redirect')) { [IO.Directory]::Delete((Join-Path $payload 'redirect')) }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
