# Downloads the pinned Slang compiler for this host (ADR-0020 §4c); the Windows twin of
# scripts/fetch-slang.sh (PowerShell 5.1+ or pwsh 7 on any OS).
#
# Usage:
#   scripts/fetch-slang.ps1           # fetch, verify and unpack (no-op when already present)
#   scripts/fetch-slang.ps1 -Print    # print the slangc path it would use, fetch nothing
#
# The version and the SHA-256 of each host archive come from crates/shaders/slang-version.txt.
# The archive is downloaded only from the official release
# (https://github.com/shader-slang/slang/releases), checked against that SHA-256, and unpacked
# into $env:CARGO_TARGET_DIR (default target)\tools\slang-<version>\, where
# crates/shaders/build.rs looks for it. Nothing is installed system-wide.
#
# Idempotent and offline-friendly: an unpacked toolchain whose recorded checksum matches the
# pin is reused without network access, and a previously downloaded archive in
# tools\downloads\ is reused once its checksum is verified again.
[CmdletBinding()]
param([switch]$Print)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
$Pin = Join-Path $Root 'crates/shaders/slang-version.txt'
$Releases = 'https://github.com/shader-slang/slang/releases/download'

function Fail([string]$Message) {
    Write-Error "fetch-slang: error: $Message" -ErrorAction Continue
    exit 1
}

$lines = Get-Content -LiteralPath $Pin
$versionLine = $lines | Where-Object { $_ -match '^version=(\S+)' } | Select-Object -First 1
if (-not $versionLine) { Fail "no version= line in $Pin" }
$version = ($versionLine -replace '^version=', '').Trim()

# Host: Windows unless running pwsh on Linux/macOS ($IsLinux/$IsMacOS exist only in pwsh 6+).
$isLinuxHost = (Test-Path variable:IsLinux) -and $IsLinux
$isMacHost = (Test-Path variable:IsMacOS) -and $IsMacOS
$os = if ($isLinuxHost) { 'linux' } elseif ($isMacHost) { 'macos' } else { 'windows' }
$machine = if ($os -eq 'windows') { $env:PROCESSOR_ARCHITECTURE } else { (& uname -m) }
switch -Regex ($machine) {
    '^(AMD64|x86_64|amd64)$' { $arch = 'x86_64'; break }
    '^(ARM64|aarch64|arm64)$' { $arch = 'aarch64'; break }
    default { Fail "unsupported host architecture $machine" }
}
$hostName = "$os-$arch"

$entry = $lines | Where-Object { ($_ -split '\s+')[0] -eq $hostName } | Select-Object -First 1
if (-not $entry) { Fail "no pinned archive for host $hostName in $Pin" }
$fields = $entry -split '\s+'
$archive = $fields[1]
$expected = $fields[2].ToLowerInvariant()

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$tools = Join-Path $targetDir 'tools'
$dest = Join-Path $tools "slang-$version"
$exe = if ($os -eq 'windows') { 'slangc.exe' } else { 'slangc' }
$slangc = Join-Path (Join-Path $dest 'bin') $exe
$marker = Join-Path $dest '.archive-sha256'

if ($Print) {
    Write-Output $slangc
    exit 0
}

# Already unpacked from an archive with the pinned checksum: nothing to do, no network.
if ((Test-Path -LiteralPath $slangc) -and (Test-Path -LiteralPath $marker) -and
    ((Get-Content -LiteralPath $marker -Raw).Trim() -eq $expected)) {
    Write-Output "fetch-slang: slangc $version already present at $slangc"
    exit 0
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

$downloads = Join-Path $tools 'downloads'
New-Item -ItemType Directory -Force -Path $downloads | Out-Null
$download = Join-Path $downloads $archive
if ((Test-Path -LiteralPath $download) -and ((Get-Sha256 $download) -eq $expected)) {
    Write-Output "fetch-slang: reusing verified $download"
} else {
    $url = "$Releases/v$version/$archive"
    Write-Output "fetch-slang: downloading $url"
    $partial = "$download.partial"
    Remove-Item -LiteralPath $partial -Force -ErrorAction SilentlyContinue
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is very slow with progress
    $attempt = 0
    while ($true) {
        try {
            Invoke-WebRequest -Uri $url -OutFile $partial -UseBasicParsing
            break
        } catch {
            $attempt++
            if ($attempt -ge 3) { Fail "download failed: $url ($_)" }
            Start-Sleep -Seconds (2 * $attempt)
        }
    }
    $actual = Get-Sha256 $partial
    if ($actual -ne $expected) {
        Remove-Item -LiteralPath $partial -Force
        Fail "checksum mismatch for ${archive}: expected $expected, got $actual"
    }
    Move-Item -LiteralPath $partial -Destination $download -Force
}

# Unpack into a staging directory next to the destination, then move it into place, so an
# interrupted run never leaves a half-unpacked toolchain that looks complete.
$staging = Join-Path $tools ".slang-$version.staging"
Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $staging | Out-Null
if ($archive.EndsWith('.zip')) {
    Expand-Archive -LiteralPath $download -DestinationPath $staging -Force
} elseif ($archive.EndsWith('.tar.gz')) {
    & tar -xzf $download -C $staging
    if ($LASTEXITCODE -ne 0) { Fail "tar failed on $download" }
} else {
    Fail "unknown archive type: $archive"
}
# Releases unpack to bin/, lib/, include/ at the top level; tolerate one wrapping directory.
$unpacked = $staging
if (-not (Test-Path -LiteralPath (Join-Path $staging 'bin'))) {
    $inner = @(Get-ChildItem -LiteralPath $staging -Directory)
    if ($inner.Count -eq 1 -and (Test-Path -LiteralPath (Join-Path $inner[0].FullName 'bin'))) {
        $unpacked = $inner[0].FullName
    }
}
if (-not (Test-Path -LiteralPath (Join-Path (Join-Path $unpacked 'bin') $exe))) {
    Fail "$archive has no bin/$exe"
}
Set-Content -LiteralPath (Join-Path $unpacked '.archive-sha256') -Value $expected -NoNewline
Remove-Item -LiteralPath $dest -Recurse -Force -ErrorAction SilentlyContinue
Move-Item -LiteralPath $unpacked -Destination $dest
Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue

Write-Output "fetch-slang: slangc $version installed at $slangc"
& $slangc -version
