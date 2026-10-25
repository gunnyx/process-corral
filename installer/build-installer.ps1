<#
.SYNOPSIS
  Builds the release exes and the Process Corral installer.

.DESCRIPTION
  1. cargo build --release --features gui     (skip with -SkipBuild)
  2. reads the version from Cargo.toml
  3. compiles installer\process-corral.iss with Inno Setup 6
  4. writes dist\ProcessCorral-Setup-<version>.exe and a SHA256 file beside it

  Needs Inno Setup 6. If it is missing:  winget install --id JRSoftware.InnoSetup -e
  The installer is NOT code-signed (see README notes about SmartScreen).

.PARAMETER SkipBuild
  Reuse the exes already in target\release.
#>
[CmdletBinding()]
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# --- version from Cargo.toml ([package] section only)
$inPackage = $false; $version = $null
foreach ($line in Get-Content (Join-Path $root 'Cargo.toml')) {
    if ($line -match '^\s*\[(.+)\]\s*$') { $inPackage = ($Matches[1] -eq 'package'); continue }
    if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $version = $Matches[1]; break }
}
if (-not $version) { throw 'Could not read the package version from Cargo.toml' }
Write-Host "Version: $version"

# --- Inno Setup compiler
$candidates = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) }
if (-not $candidates) {
    throw "Inno Setup 6 was not found. Install it with:`n    winget install --id JRSoftware.InnoSetup -e`nthen run this script again."
}
$iscc = $candidates | Select-Object -First 1
Write-Host "Inno Setup: $iscc"

# --- build the exes
if (-not $SkipBuild) {
    Write-Host 'cargo build --release --features gui'
    cargo build --release --features gui
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}
foreach ($exe in 'process-corral.exe', 'process-corral-gui.exe') {
    if (-not (Test-Path (Join-Path $root "target\release\$exe"))) { throw "target\release\$exe is missing" }
}

# --- compile the installer
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
& $iscc "/DAppVersion=$version" (Join-Path $root 'installer\process-corral.iss')
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed ($LASTEXITCODE)" }

$setup = Join-Path $dist "ProcessCorral-Setup-$version.exe"
if (-not (Test-Path $setup)) { throw "Expected $setup was not produced" }

# --- checksum file in `sha256sum` format
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
$sumFile = "$setup.sha256"
[IO.File]::WriteAllText($sumFile, "$hash *$(Split-Path -Leaf $setup)`n")

Write-Host ''
Write-Host "Installer : $setup"
Write-Host ("Size      : {0:N0} bytes ({1:N1} MB)" -f (Get-Item $setup).Length, ((Get-Item $setup).Length / 1MB))
Write-Host "SHA256    : $hash"
Write-Host "Checksum  : $sumFile"
