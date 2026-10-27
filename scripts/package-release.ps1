param(
    [switch]$Offline,
    [switch]$SkipBuild,
    [string]$PackageDirectory,
    [string]$OutputDirectory,
    [string]$NsisCompiler
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'release-tools.ps1')
$version = Get-ReleaseVersion -ProjectRoot $projectRoot
if (-not $PackageDirectory) { $PackageDirectory = "dist\$version" }
if (-not $OutputDirectory) { $OutputDirectory = "dist\releases\$version" }
if (-not [IO.Path]::IsPathRooted($PackageDirectory)) { $PackageDirectory = Join-Path $projectRoot $PackageDirectory }
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) { $OutputDirectory = Join-Path $projectRoot $OutputDirectory }
Assert-SeparateReleaseDirectories -Payload $PackageDirectory -Output $OutputDirectory
if (-not $NsisCompiler) {
    $command = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($command) { $NsisCompiler = $command.Source }
    else { $NsisCompiler = Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe' }
}
$NsisCompiler = (Resolve-Path -LiteralPath $NsisCompiler -ErrorAction Stop).ProviderPath
$nsisNotices = Join-Path (Split-Path -Parent $NsisCompiler) 'COPYING'
if (-not (Test-Path -LiteralPath $nsisNotices -PathType Leaf)) { throw 'NSIS COPYING notices are required beside the compiler.' }
if (-not $SkipBuild) { & (Join-Path $PSScriptRoot 'build.ps1') -Offline:$Offline -PackageDirectory $PackageDirectory }
$files = @(Assert-ReleasePayload -Directory $PackageDirectory -Version $version)
$packageRoot = (Resolve-Path -LiteralPath $PackageDirectory).ProviderPath.TrimEnd('\', '/')
$stage = Join-Path $projectRoot ('target\release-packaging\' + [guid]::NewGuid().ToString('N'))
$payload = Join-Path $stage 'payload'
New-Item -ItemType Directory -Path $payload -Force | Out-Null
foreach ($file in $files) {
    $relative = $file.FullName.Substring($packageRoot.Length + 1)
    $destination = Join-Path $payload $relative
    New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
    Copy-Item -LiteralPath $file.FullName -Destination $destination
}
# Revalidate the staged snapshot; neither archive nor installer reads the live build.
$stagedFiles = @(Assert-ReleasePayload -Directory $payload -Version $version)
$include = Join-Path $stage 'payload.nsh'
Write-ReleasePayloadInclude -Directory $payload -Destination $include
$installerName = "DiskScope-$version-windows-x64-setup.exe"
$archiveName = "DiskScope-$version-windows-x64-portable.zip"
$installer = Join-Path $stage $installerName
$archive = Join-Path $stage $archiveName
$numericVersion = ($version -split '[-+]')[0] + '.0'
$sizeKiB = [math]::Ceiling(($stagedFiles | Measure-Object -Property Length -Sum).Sum / 1KB)
& $NsisCompiler '/V2' '/WX' "/DAPP_VERSION=$version" "/DPRODUCT_VERSION=$numericVersion" "/DOUTPUT_FILE=$installer" "/DPAYLOAD_INCLUDE=$include" "/DESTIMATED_SIZE=$sizeKiB" "/DNSIS_NOTICES=$nsisNotices" (Join-Path $projectRoot 'packaging\DiskScope.nsi')
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $installer)) { throw 'Installer compilation failed.' }
New-PortableArchive -Directory $payload -Destination $archive -Version $version
Copy-Item -LiteralPath $nsisNotices -Destination (Join-Path $stage 'INSTALLER_NOTICES.txt')
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$hashes = foreach ($name in @($installerName, $archiveName, 'INSTALLER_NOTICES.txt')) {
    $source = Join-Path $stage $name
    $published = Join-Path $OutputDirectory $name
    Copy-Item -LiteralPath $source -Destination $published -Force
    $sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    $publishedHash = (Get-FileHash -LiteralPath $published -Algorithm SHA256).Hash
    if ($sourceHash -cne $publishedHash) { throw "Published package differs from the build snapshot: $name" }
    $publishedHash.ToLowerInvariant() + '  ' + $name
}
[IO.File]::WriteAllLines((Join-Path $OutputDirectory 'SHA256SUMS.txt'), [string[]]$hashes, [Text.UTF8Encoding]::new($false))
Write-Output "Installer: $(Join-Path $OutputDirectory $installerName)"
Write-Output "Portable ZIP: $(Join-Path $OutputDirectory $archiveName)"
Write-Output "Unpacked portable: $PackageDirectory"
Write-Output "Build snapshot: $stage"
