# Downloads and verifies the pinned vendor SDKs behind moba-render's optional features
# (ADR-0020 §6b); the Windows twin of scripts/fetch-sdks.sh (PowerShell 5.1+ or pwsh 7).
# Nothing is installed system-wide and nothing from an archive is executed.
#
# Usage:
#   scripts/fetch-sdks.ps1                      # every verified entry in vendor-sdks.toml
#   scripts/fetch-sdks.ps1 -Only dlss,nrd       # just these features or entry names
#   scripts/fetch-sdks.ps1 -Only vendor-sdks    # the aggregates nvidia, vendor-sdks, dlss-fg too
#   scripts/fetch-sdks.ps1 -PresentFeatures     # print the features whose SDKs are all present
#
# Each archive comes from the URL pinned in vendor-sdks.toml (an official GitHub release asset
# or tag/commit archive), is checked against its SHA-256 and unpacked into
# $env:MOBA_SDK_DIR (default target\vendor-sdks)\<dir>\. Git submodules and CMake downloads are
# entries of their own, unpacked inside their parent. A changed licence text (its SHA-256)
# fails the fetch, which forces the re-read roadmap §7 asks for at each bump.
[CmdletBinding()]
param(
    [string[]]$Only = @(),
    [switch]$PresentFeatures
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
$Manifest = Join-Path $Root 'vendor-sdks.toml'
$SdkDir = if ($env:MOBA_SDK_DIR) { $env:MOBA_SDK_DIR } else { Join-Path (Join-Path $Root 'target') 'vendor-sdks' }
$StampName = '.moba-sdk-sha256'

function Fail([string]$Message) {
    Write-Error "fetch-sdks: error: $Message" -ErrorAction Continue
    exit 1
}
function Say([string]$Message) { Write-Output "fetch-sdks: $Message" }
function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

if (-not (Test-Path -LiteralPath $Manifest)) { Fail "missing $Manifest" }

# The flat subset vendor-sdks.toml uses: [[sdk]] tables of `key = "string"` and
# `key = ["a", "b"]` lines.
$entries = New-Object System.Collections.ArrayList
$current = $null
foreach ($line in Get-Content -LiteralPath $Manifest) {
    if ($line -match '^\[\[sdk\]\]') {
        $current = @{}
        [void]$entries.Add($current)
        continue
    }
    if ($line -match '^\[') { $current = $null; continue }
    if ($null -eq $current) { continue }
    if ($line -match '^([a-z_0-9]+)\s*=\s*(.*)$') {
        $key = $Matches[1]
        $value = $Matches[2].Trim()
        if ($value.StartsWith('[')) {
            $current[$key] = @(($value.Trim('[', ']') -split ',') |
                ForEach-Object { $_.Trim().Trim('"') } | Where-Object { $_ })
        } elseif ($value -match '^"([^"]*)"') {
            $current[$key] = $Matches[1]
        }
    }
}
if ($entries.Count -eq 0) { Fail "no [[sdk]] entries in $Manifest" }
function Field($Entry, [string]$Key) { if ($Entry.ContainsKey($Key)) { $Entry[$Key] } else { '' } }

# Sub-entries inherit their parent's features.
$featuresOf = @{}
foreach ($e in $entries) {
    $parent = Field $e 'parent'
    $featuresOf[$e.name] = if ($parent) { $featuresOf[$parent] } else { @(Field $e 'features') }
}

$wanted = @()
foreach ($item in ($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })) {
    switch ($item) {
        'nvidia' { $wanted += 'dlss', 'nrd', 'rtxdi', 'sharc' }
        'vendor-sdks' { $wanted += 'dlss', 'nrd', 'rtxdi', 'sharc', 'fsr4', 'xess' }
        'dlss-fg' { $wanted += 'dlss', 'streamline' }
        default { $wanted += $item }
    }
}

function Test-Present([string]$Dest, $Entry) {
    $stamp = Join-Path $Dest $StampName
    $licence = Join-Path $Dest $Entry.licence
    (Test-Path -LiteralPath $stamp) -and
    ((Get-Content -LiteralPath $stamp -Raw).Trim() -eq $Entry.sha256) -and
    (Test-Path -LiteralPath $licence) -and
    ((Get-Sha256 $licence) -eq $Entry.licence_sha256)
}

if ($PresentFeatures) {
    $known = @{}
    $missing = @{}
    foreach ($e in $entries) {
        $ok = ((Field $e 'status') -eq 'verified') -and (Test-Present (Join-Path $SdkDir $e.dir) $e)
        foreach ($f in $featuresOf[$e.name]) {
            $known[$f] = $true
            if (-not $ok) { $missing[$f] = $true }
        }
    }
    $out = @('dlss', 'streamline', 'nrd', 'rtxdi', 'sharc', 'fsr4', 'xess') |
        Where-Object { $known.ContainsKey($_) -and -not $missing.ContainsKey($_) }
    Write-Output ($out -join ',')
    exit 0
}

$selected = @{}
function Test-Selected($Entry) {
    if ($wanted.Count -eq 0) { return $true }
    if ($wanted -contains $Entry.name) { return $true }
    $parent = Field $Entry 'parent'
    if ($parent -and $selected.ContainsKey($parent)) { return $true }
    foreach ($f in $featuresOf[$Entry.name]) { if ($wanted -contains $f) { return $true } }
    return $false
}

$downloads = Join-Path $SdkDir 'downloads'
New-Item -ItemType Directory -Force -Path $downloads | Out-Null
$tar = Get-Command tar.exe -ErrorAction SilentlyContinue
$fetched = 0
$skipped = 0
foreach ($e in $entries) {
    if (-not (Test-Selected $e)) { continue }
    $selected[$e.name] = $true
    $name = $e.name
    $dest = Join-Path $SdkDir $e.dir
    if ((Field $e 'status') -ne 'verified') {
        Say "skipping ${name}: marked $(Field $e 'status') in vendor-sdks.toml (see docs/dev/vendor-sdks.md)"
        $skipped++
        continue
    }
    foreach ($k in 'url', 'sha256', 'dir', 'licence', 'licence_sha256') {
        if (-not (Field $e $k)) { Fail "${name}: incomplete entry in $Manifest ($k)" }
    }
    if (-not $e.url.StartsWith('https://github.com/')) { Fail "${name}: $($e.url) is not a GitHub URL" }
    $digest = Field $e 'github_digest'
    if ($digest -and $digest -ne "sha256:$($e.sha256)") { Fail "${name}: github_digest $digest does not match sha256" }

    if (Test-Present $dest $e) { Say "${name}: present at $dest"; continue }

    $download = Join-Path $downloads ("$name-" + ($e.url -split '/')[-1])
    if ((Test-Path -LiteralPath $download) -and ((Get-Sha256 $download) -eq $e.sha256)) {
        Say "${name}: reusing verified $download"
    } else {
        Say "${name}: downloading $($e.url)"
        $partial = "$download.partial"
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force }
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -Uri $e.url -OutFile $partial -UseBasicParsing
        $actual = Get-Sha256 $partial
        if ($actual -ne $e.sha256) {
            Remove-Item -LiteralPath $partial -Force
            Fail "${name}: checksum mismatch for $($e.url): expected $($e.sha256), got $actual"
        }
        Move-Item -LiteralPath $partial -Destination $download -Force
    }

    $staging = Join-Path $SdkDir ".staging-$name"
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    # bsdtar (tar.exe, Windows 10+) unpacks zip archives far faster than Expand-Archive.
    if ($tar) { & $tar.Source -xf $download -C $staging; if ($LASTEXITCODE -ne 0) { Fail "${name}: unpacking failed" } }
    else { Expand-Archive -LiteralPath $download -DestinationPath $staging -Force }
    $root = if (Field $e 'strip') { Join-Path $staging $e.strip } else { $staging }
    if (-not (Test-Path -LiteralPath $root)) { Fail "${name}: the archive has no top-level directory $($e.strip)" }
    $licence = Join-Path $root $e.licence
    if (-not (Test-Path -LiteralPath $licence)) { Fail "${name}: the archive has no licence file $($e.licence)" }
    $actual = Get-Sha256 $licence
    if ($actual -ne $e.licence_sha256) {
        $kept = Join-Path $SdkDir ("changed-licence-$name-" + (Split-Path -Leaf $e.licence))
        Copy-Item -LiteralPath $licence -Destination $kept -Force
        Remove-Item -LiteralPath $staging -Recurse -Force
        Fail "${name}: the licence text changed ($($e.licence): expected $($e.licence_sha256), got $actual). Re-read it (roadmap §7; copy at $kept), record the re-read in the PR and update licence_sha256 in vendor-sdks.toml"
    }
    # No byte-order mark: build.rs and the shell script compare the stamp's text.
    [IO.File]::WriteAllText((Join-Path $root $StampName), "$($e.sha256)`n")
    if (Test-Path -LiteralPath $dest) { Remove-Item -LiteralPath $dest -Recurse -Force }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
    Move-Item -LiteralPath $root -Destination $dest
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    Say "${name}: verified archive and licence; unpacked to $dest (binaries for: $((Field $e 'platforms') -join ', '))"
    $fetched++
}

if ($wanted.Count -gt 0 -and $selected.Count -eq 0) { Fail "-Only $($Only -join ',') matched no entry in $Manifest" }
Say "done: $fetched fetched, $skipped skipped, SDK dir $SdkDir"
