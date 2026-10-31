<#
Installs cargo-deny from a digest-pinned source package, the way the NSIS
compiler is pinned: exact byte size and SHA-256 recorded here, never learned
from a response. The checker runs before anything else validates dependencies,
so its own provenance must not depend on live index resolution. Cargo verifies
every dependency against the checksums in the package's own Cargo.lock, which
is itself covered by the recorded crate digest.
#>
param(
    [string]$Version = '0.20.2',
    [long]$ExpectedBytes = 217654,
    [string]$ExpectedSha256 = 'e528dfcbe739af7ce37a77d3d6df1b29dd6887b1c701d888820c0f16b864f737'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$crateName = "cargo-deny-$Version.crate"
$crate = Join-Path ([IO.Path]::GetTempPath()) ('diskscope-' + $crateName)
if (Test-Path -LiteralPath $crate) { throw "Refusing to reuse an existing download: $crate" }
$source = Join-Path ([IO.Path]::GetTempPath()) ('diskscope-cargo-deny-' + $Version + '-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $source | Out-Null
try {
    & curl.exe --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --connect-timeout 20 --max-time 120 --output $crate "https://static.crates.io/crates/cargo-deny/$crateName"
    if ($LASTEXITCODE -ne 0) { throw "cargo-deny download failed with curl exit $LASTEXITCODE; package not trusted or installed." }
    $length = (Get-Item -LiteralPath $crate).Length
    $hash = (Get-FileHash -LiteralPath $crate -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($length -ne $ExpectedBytes -or $hash -cne $ExpectedSha256) {
        throw "cargo-deny $Version package is $length bytes with SHA-256 $hash; expected $ExpectedBytes bytes with $ExpectedSha256. Package not trusted or installed."
    }
    # The system tar is addressed by absolute path: a Git-for-Windows GNU tar
    # earlier on PATH would misread Windows paths as remote-host specs.
    $systemTar = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (-not (Test-Path -LiteralPath $systemTar -PathType Leaf)) { throw 'Windows system tar.exe was not found.' }
    & $systemTar -xf $crate -C $source
    if ($LASTEXITCODE -ne 0) { throw 'cargo-deny package extraction failed.' }
    cargo install --path (Join-Path $source "cargo-deny-$Version") --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $installed = & cargo deny --version
    if ($LASTEXITCODE -ne 0 -or $installed -cne "cargo-deny $Version") { throw "Installed cargo-deny is '$installed'; expected cargo-deny $Version." }
    Write-Output "cargo-deny $Version installed from the digest-pinned package ($hash)."
} finally {
    Remove-Item -LiteralPath $crate -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $source -Recurse -Force -ErrorAction SilentlyContinue
}
