#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PWSH_IMAGE="${MAYHEM_DOCKER_PWSH_IMAGE:-mcr.microsoft.com/powershell:7.4-debian-12}"
STAGED_ROOT=""
CHECK_SCRIPT=""

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '==> %s\n' "$*"
}

usage() {
  cat <<'USAGE'
Usage: scripts/docker-powershell-installer-check.sh

Run PowerShell installer checks in Docker:
  - parse install.ps1 with PowerShell Core
  - install a synthetic artifact whose checked package root competes with an
    unchecked shadow binary elsewhere in the archive
  - reject an archive containing multiple SHA256SUMS files
  - reject a SHA256SUMS file containing duplicate relative paths
  - reject unsafe dot or empty path segments in SHA256SUMS entries

This is local P8.1 PowerShell installer evidence. It does not replace the
formal Windows clean-VM install gate.

Environment:
  MAYHEM_DOCKER_PWSH_IMAGE  PowerShell image
                            (default: mcr.microsoft.com/powershell:7.4-debian-12)
  MAYHEM_DOCKER_KEEP_STAGE  Keep temporary staged checkout when set to 1
USAGE
}

cleanup() {
  if [[ -n "$STAGED_ROOT" && "${MAYHEM_DOCKER_KEEP_STAGE:-0}" != "1" ]]; then
    rm -rf "$STAGED_ROOT"
  fi
  if [[ -n "$CHECK_SCRIPT" ]]; then
    rm -f "$CHECK_SCRIPT"
  fi
}
trap cleanup EXIT

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v docker >/dev/null 2>&1 || die "docker is required"

WORK_ROOT="$ROOT_DIR"
if ! docker run --rm -v "$ROOT_DIR:/work:ro" "$PWSH_IMAGE" pwsh -NoProfile -NonInteractive -Command 'if (-not (Test-Path /work/install.ps1)) { exit 1 }' >/dev/null 2>&1; then
  command -v rsync >/dev/null 2>&1 || die "Docker cannot mount $ROOT_DIR and rsync is unavailable for staging"
  STAGED_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-docker-work.XXXXXX")"
  log "Docker cannot mount $ROOT_DIR; staging checkout at $STAGED_ROOT"
  rsync -a --delete \
    --exclude .git \
    --exclude target \
    --exclude dist \
    --exclude node_modules \
    --exclude stores \
    "$ROOT_DIR/" "$STAGED_ROOT/"
  WORK_ROOT="$STAGED_ROOT"
fi

CHECK_SCRIPT="$(mktemp "${TMPDIR:-/tmp}/mayhem-pwsh-installer-check.XXXXXX.ps1")"
cat > "$CHECK_SCRIPT" <<'PS'
$ErrorActionPreference = "Stop"
if ($PSStyle) {
    $PSStyle.OutputRendering = "PlainText"
}

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Invoke-CheckedProcess {
    param(
        [string]$FileName,
        [string[]]$Arguments
    )

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $FileName
    foreach ($arg in $Arguments) {
        [void]$psi.ArgumentList.Add($arg)
    }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false

    $process = [System.Diagnostics.Process]::Start($psi)
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()

    return [pscustomobject]@{
        ExitCode = $process.ExitCode
        Stdout = $stdout
        Stderr = $stderr
        Text = ($stdout + $stderr)
    }
}

function New-ExecutableScript {
    param(
        [string]$Path,
        [string]$Name,
        [int]$ExitCode = 0
    )

    $content = @'
#!/bin/sh
if [ "${1:-}" = "--help" ]; then
  echo "__HELP__"
  exit __EXIT__
fi
echo "__RUN__"
exit __EXIT__
'@
    $content = $content.Replace("__HELP__", "verified $Name help")
    $content = $content.Replace("__RUN__", "verified $Name")
    $content = $content.Replace("__EXIT__", "$ExitCode")
    Set-Content -Path $Path -Value $content -NoNewline -Encoding utf8
    & chmod 0755 $Path
    if ($LASTEXITCODE -ne 0) {
        throw "chmod failed for $Path"
    }
}

function New-ArchiveHash {
    param([string]$Path)
    return (Get-FileHash -Algorithm SHA256 -Path $Path).Hash.ToLowerInvariant()
}

function New-InstallerArtifact {
    param(
        [string]$Root,
        [switch]$IncludeShadow,
        [switch]$IncludeSecondSums,
        [switch]$IncludeDuplicatePath,
        [switch]$IncludeDotPath,
        [switch]$IncludeEmptySegmentPath
    )

    $bins = @(
        "mayhem",
        "mayhem-gateway",
        "mayhem-pay",
        "mayhemd",
        "mayhem-enclave",
        "mayhem-paygate"
    )

    $package = Join-Path $Root "mayhem-pwsh-test-aarch64-pc-windows-msvc"
    $binDir = Join-Path $package "bin"
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null

    foreach ($bin in $bins) {
        New-ExecutableScript -Path (Join-Path $binDir "$bin.exe") -Name $bin
    }

    $sums = foreach ($bin in $bins) {
        $file = Join-Path $binDir "$bin.exe"
        "$(New-ArchiveHash -Path $file)  bin/$bin.exe"
    }
    if ($IncludeDuplicatePath) {
        $sums += $sums[0]
    }
    if ($IncludeDotPath) {
        $mayhem = Join-Path $binDir "mayhem.exe"
        $sums[0] = "$(New-ArchiveHash -Path $mayhem)  bin/./mayhem.exe"
    }
    if ($IncludeEmptySegmentPath) {
        $mayhem = Join-Path $binDir "mayhem.exe"
        $sums[0] = "$(New-ArchiveHash -Path $mayhem)  bin//mayhem.exe"
    }
    Set-Content -Path (Join-Path $package "SHA256SUMS") -Value $sums -Encoding ascii

    $entries = @((Split-Path -Leaf $package))
    if ($IncludeShadow) {
        $shadowDir = Join-Path $Root "000/bin"
        New-Item -ItemType Directory -Path $shadowDir -Force | Out-Null
        New-ExecutableScript -Path (Join-Path $shadowDir "mayhem.exe") -Name "unchecked shadow mayhem" -ExitCode 42
        $entries = @("000") + $entries
    }
    if ($IncludeSecondSums) {
        $secondDir = Join-Path $Root "001"
        New-Item -ItemType Directory -Path $secondDir -Force | Out-Null
        Set-Content -Path (Join-Path $secondDir "SHA256SUMS") -Value ("0" * 64 + "  nope") -Encoding ascii
        $entries = @("001") + $entries
    }

    $archive = Join-Path $Root "mayhem-pwsh-test.tar.gz"
    $tarArgs = @("-czf", $archive, "-C", $Root) + $entries
    & tar @tarArgs
    if ($LASTEXITCODE -ne 0) {
        throw "tar failed to create $archive"
    }

    return [pscustomobject]@{
        Archive = $archive
        Sha256 = New-ArchiveHash -Path $archive
    }
}

$tokens = $null
$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile("/work/install.ps1", [ref]$tokens, [ref]$errors) | Out-Null
if ($errors.Count -gt 0) {
    foreach ($parseError in $errors) {
        Write-Error $parseError.Message
    }
    exit 1
}
Write-Step "parsed install.ps1"
Write-Step "starting synthetic artifact checks"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("mayhem-pwsh-check-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    $artifactRoot = Join-Path $tmp "shadow"
    New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null
    $artifact = New-InstallerArtifact -Root $artifactRoot -IncludeShadow
    Write-Step "created shadow artifact"
    $installDir = Join-Path $tmp "install/bin"

    $result = Invoke-CheckedProcess -FileName "pwsh" -Arguments @(
        "-NoProfile",
        "-NonInteractive",
        "-File",
        "/work/install.ps1",
        "-Artifact",
        $artifact.Archive,
        "-Sha256",
        $artifact.Sha256,
        "-InstallDir",
        $installDir,
        "-SkipPear",
        "-SkipOpencode",
        "-NoPathUpdate"
    )
    Write-Host $result.Text
    if ($result.ExitCode -ne 0) {
        throw "install.ps1 failed for shadow artifact"
    }
    foreach ($expected in @(
        "verified archive SHA-256",
        "verified 6 packaged file checksum(s)",
        "Copy/paste PATH for this PowerShell session:",
        "mayhem CLI smoke test passed"
    )) {
        if (-not $result.Text.Contains($expected)) {
            throw "install.ps1 output missing: $expected"
        }
    }

    $mayhem = Join-Path $installDir "mayhem.exe"
    $help = Invoke-CheckedProcess -FileName $mayhem -Arguments @("--help")
    if ($help.ExitCode -ne 0 -or -not $help.Text.Contains("verified mayhem help")) {
        throw "installed mayhem.exe did not come from the verified package root"
    }
    Write-Step "shadow artifact installed verified package binaries"

    $multiRoot = Join-Path $tmp "multi"
    New-Item -ItemType Directory -Path $multiRoot -Force | Out-Null
    $multi = New-InstallerArtifact -Root $multiRoot -IncludeSecondSums
    Write-Step "created multiple-SHA256SUMS artifact"
    $negative = Invoke-CheckedProcess -FileName "pwsh" -Arguments @(
        "-NoProfile",
        "-NonInteractive",
        "-File",
        "/work/install.ps1",
        "-Artifact",
        $multi.Archive,
        "-Sha256",
        $multi.Sha256,
        "-InstallDir",
        (Join-Path $tmp "multi-install/bin"),
        "-SkipPear",
        "-SkipOpencode",
        "-NoPathUpdate"
    )
    Write-Host $negative.Text
    if ($negative.ExitCode -eq 0) {
        throw "install.ps1 unexpectedly accepted multiple SHA256SUMS files"
    }
    if (-not $negative.Text.Contains("artifact contains multiple SHA256SUMS files")) {
        throw "multiple SHA256SUMS rejection message was missing"
    }
    Write-Step "multiple SHA256SUMS archive rejected"

    $duplicateRoot = Join-Path $tmp "duplicate-path"
    New-Item -ItemType Directory -Path $duplicateRoot -Force | Out-Null
    $duplicate = New-InstallerArtifact -Root $duplicateRoot -IncludeDuplicatePath
    Write-Step "created duplicate-path SHA256SUMS artifact"
    $duplicateResult = Invoke-CheckedProcess -FileName "pwsh" -Arguments @(
        "-NoProfile",
        "-NonInteractive",
        "-File",
        "/work/install.ps1",
        "-Artifact",
        $duplicate.Archive,
        "-Sha256",
        $duplicate.Sha256,
        "-InstallDir",
        (Join-Path $tmp "duplicate-install/bin"),
        "-SkipPear",
        "-SkipOpencode",
        "-NoPathUpdate"
    )
    Write-Host $duplicateResult.Text
    if ($duplicateResult.ExitCode -eq 0) {
        throw "install.ps1 unexpectedly accepted duplicate SHA256SUMS paths"
    }
    if (-not $duplicateResult.Text.Contains("SHA256SUMS contains duplicate path: bin/mayhem.exe")) {
        throw "duplicate SHA256SUMS path rejection message was missing"
    }
    Write-Step "duplicate SHA256SUMS path rejected"

    $dotRoot = Join-Path $tmp "dot-path"
    New-Item -ItemType Directory -Path $dotRoot -Force | Out-Null
    $dot = New-InstallerArtifact -Root $dotRoot -IncludeDotPath
    Write-Step "created dot-path SHA256SUMS artifact"
    $dotResult = Invoke-CheckedProcess -FileName "pwsh" -Arguments @(
        "-NoProfile",
        "-NonInteractive",
        "-File",
        "/work/install.ps1",
        "-Artifact",
        $dot.Archive,
        "-Sha256",
        $dot.Sha256,
        "-InstallDir",
        (Join-Path $tmp "dot-install/bin"),
        "-SkipPear",
        "-SkipOpencode",
        "-NoPathUpdate"
    )
    Write-Host $dotResult.Text
    if ($dotResult.ExitCode -eq 0) {
        throw "install.ps1 unexpectedly accepted dot-segment SHA256SUMS path"
    }
    if (-not $dotResult.Text.Contains("unsafe SHA256SUMS path: bin/./mayhem.exe")) {
        throw "dot-segment SHA256SUMS path rejection message was missing"
    }
    Write-Step "dot-segment SHA256SUMS path rejected"

    $emptySegmentRoot = Join-Path $tmp "empty-segment-path"
    New-Item -ItemType Directory -Path $emptySegmentRoot -Force | Out-Null
    $emptySegment = New-InstallerArtifact -Root $emptySegmentRoot -IncludeEmptySegmentPath
    Write-Step "created empty-segment SHA256SUMS artifact"
    $emptySegmentResult = Invoke-CheckedProcess -FileName "pwsh" -Arguments @(
        "-NoProfile",
        "-NonInteractive",
        "-File",
        "/work/install.ps1",
        "-Artifact",
        $emptySegment.Archive,
        "-Sha256",
        $emptySegment.Sha256,
        "-InstallDir",
        (Join-Path $tmp "empty-segment-install/bin"),
        "-SkipPear",
        "-SkipOpencode",
        "-NoPathUpdate"
    )
    Write-Host $emptySegmentResult.Text
    if ($emptySegmentResult.ExitCode -eq 0) {
        throw "install.ps1 unexpectedly accepted empty-segment SHA256SUMS path"
    }
    if (-not $emptySegmentResult.Text.Contains("unsafe SHA256SUMS path: bin//mayhem.exe")) {
        throw "empty-segment SHA256SUMS path rejection message was missing"
    }
    Write-Step "empty-segment SHA256SUMS path rejected"
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
PS

log "running PowerShell installer checks in $PWSH_IMAGE"
docker run --rm \
  -v "$WORK_ROOT:/work:ro" \
  -v "$CHECK_SCRIPT:/tmp/mayhem-pwsh-installer-check.ps1:ro" \
  "$PWSH_IMAGE" \
  pwsh -NoProfile -NonInteractive -File /tmp/mayhem-pwsh-installer-check.ps1

log "PowerShell installer Docker check passed"
