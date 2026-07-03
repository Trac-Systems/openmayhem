#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$FromSource,
    [string]$SourceDir = $(if ($env:MAYHEM_SOURCE_DIR) { $env:MAYHEM_SOURCE_DIR } else { $PSScriptRoot }),
    [string]$Artifact = $env:MAYHEM_ARTIFACT,
    [string]$ArtifactUrl = $env:MAYHEM_ARTIFACT_URL,
    [string]$Sha256 = $env:MAYHEM_ARTIFACT_SHA256,
    [string]$ReleaseBaseUrl = $env:MAYHEM_RELEASE_BASE_URL,
    [string]$Version = $(if ($env:MAYHEM_VERSION) { $env:MAYHEM_VERSION } else { "latest" }),
    [string]$InstallDir = $(if ($env:MAYHEM_INSTALL_DIR) { $env:MAYHEM_INSTALL_DIR } else { Join-Path (Join-Path $HOME ".mayhem") "bin" }),
    [switch]$SkipNode,
    [switch]$SkipPear,
    [switch]$SkipOpencode,
    [string]$OpencodeVersion = $(if ($env:MAYHEM_OPENCODE_VERSION) { $env:MAYHEM_OPENCODE_VERSION } else { "1.17.13" }),
    [switch]$ForceOpencode,
    [switch]$NoPathUpdate,
    [switch]$AllowUnverified,
    [string]$NpmPrefix = $(if ($env:MAYHEM_NPM_PREFIX) { $env:MAYHEM_NPM_PREFIX } else { Join-Path (Join-Path $HOME ".mayhem") "node" })
)

$ErrorActionPreference = "Stop"

if ($env:MAYHEM_FROM_SOURCE -eq "1") { $FromSource = $true }
if ($env:MAYHEM_SKIP_NODE -eq "1") { $SkipNode = $true }
if ($env:MAYHEM_SKIP_PEAR -eq "1") { $SkipPear = $true }
if ($env:MAYHEM_SKIP_OPENCODE -eq "1") { $SkipOpencode = $true }
if ($env:MAYHEM_FORCE_OPENCODE -eq "1") { $ForceOpencode = $true }
if ($env:MAYHEM_NO_PATH_UPDATE -eq "1") { $NoPathUpdate = $true }
if ($env:MAYHEM_ALLOW_UNVERIFIED -eq "1") { $AllowUnverified = $true }

$Bins = @(
    "mayhem",
    "mayhem-gateway",
    "mayhem-pay",
    "mayhemd",
    "mayhem-enclave",
    "mayhem-paygate"
)

$script:PathEntries = @()
$script:TempDirs = @()

function Write-Log {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Write-Warn {
    param([string]$Message)
    Write-Warning $Message
}

function Fail {
    param([string]$Message)
    throw $Message
}

function Test-Command {
    param([string]$Name)
    return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function New-TempDir {
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("mayhem-install-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $script:TempDirs += $dir
    return $dir
}

function Add-PathEntry {
    param([string]$Entry)

    if ([string]::IsNullOrWhiteSpace($Entry)) {
        return
    }

    if ($script:PathEntries -notcontains $Entry) {
        $script:PathEntries += $Entry
    }

    $parts = $env:Path -split ";"
    if ($parts -notcontains $Entry) {
        $env:Path = "$Entry;$env:Path"
    }
}

function Get-TargetTriple {
    $arch = $env:PROCESSOR_ARCHITECTURE
    if ([string]::IsNullOrWhiteSpace($arch)) {
        $arch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    }

    switch -Regex ($arch) {
        "^(AMD64|X64|x86_64)$" { return "x86_64-pc-windows-msvc" }
        "^(ARM64|Arm64|AARCH64|aarch64)$" { return "aarch64-pc-windows-msvc" }
        default { Fail "unsupported Windows architecture: $arch" }
    }
}

function Get-ArchiveName {
    param([string]$Target)
    return "mayhem-$Version-$Target.zip"
}

function Test-WindowsAvx2 {
    try {
        if (-not ("MayhemProcessorFeatures" -as [type])) {
            Add-Type -TypeDefinition @"
using System.Runtime.InteropServices;
public static class MayhemProcessorFeatures {
    [DllImport("kernel32.dll", SetLastError = false)]
    public static extern bool IsProcessorFeaturePresent(uint processorFeature);
}
"@ | Out-Null
        }
        return [MayhemProcessorFeatures]::IsProcessorFeaturePresent(40)
    } catch {
        Write-Warn "could not detect AVX2 support; using the standard opencode x64 asset"
        return $true
    }
}

function Get-OpencodeAsset {
    $target = Get-TargetTriple
    switch ($target) {
        "aarch64-pc-windows-msvc" {
            return @{
                Name = "opencode-windows-arm64.zip"
                Sha256 = "bafec2dd6b89055910284ba910d59605295866563ccdb3d035c0c4b887dd11e6"
            }
        }
        "x86_64-pc-windows-msvc" {
            if (-not (Test-WindowsAvx2)) {
                return @{
                    Name = "opencode-windows-x64-baseline.zip"
                    Sha256 = "5edd43946d2bb41bb9fd975e7faefc3cb9e37a3a8fdbbfd4f1762647f92bb6b8"
                }
            }
            return @{
                Name = "opencode-windows-x64.zip"
                Sha256 = "18aa3df701a6eafcca201b5bcc63e086c96c8daa6ae2495cf718e12cb0ce3361"
            }
        }
        default {
            Fail "unsupported Windows target for opencode: $target"
        }
    }
}

function Invoke-Download {
    param(
        [string]$Uri,
        [string]$OutFile
    )

    Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing
}

function Get-SidecarHash {
    param([string]$Path)

    if (-not (Test-Path $Path)) {
        return ""
    }

    $content = Get-Content -Raw -Path $Path
    $match = [regex]::Match($content, "(?i)[0-9a-f]{64}")
    if ($match.Success) {
        return $match.Value.ToLowerInvariant()
    }
    return ""
}

function Get-ExpectedHash {
    param([string]$ArchivePath)

    if (-not [string]::IsNullOrWhiteSpace($Sha256)) {
        return $Sha256.ToLowerInvariant()
    }

    $sidecars = @(
        "$ArchivePath.sha256",
        (Join-Path (Split-Path -Parent $ArchivePath) ((Split-Path -Leaf $ArchivePath) + ".sha256"))
    )

    foreach ($sidecar in $sidecars) {
        $hash = Get-SidecarHash -Path $sidecar
        if (-not [string]::IsNullOrWhiteSpace($hash)) {
            return $hash
        }
    }

    return ""
}

function Verify-Archive {
    param([string]$ArchivePath)

    $expected = Get-ExpectedHash -ArchivePath $ArchivePath
    if ([string]::IsNullOrWhiteSpace($expected)) {
        if ($AllowUnverified) {
            Write-Warn "installing unverified archive because -AllowUnverified was set"
            return
        }
        Fail "missing checksum for $ArchivePath; pass -Sha256 or place a .sha256 sidecar next to it"
    }

    $actual = (Get-FileHash -Algorithm SHA256 -Path $ArchivePath).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Fail "checksum mismatch for $ArchivePath`: expected $expected, got $actual"
    }
    Write-Log "verified archive SHA-256 $actual"
}

function Get-ArtifactPath {
    param([string]$Target)

    if (-not [string]::IsNullOrWhiteSpace($Artifact)) {
        return (Resolve-Path $Artifact).Path
    }

    if ([string]::IsNullOrWhiteSpace($ArtifactUrl)) {
        if ([string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
            Fail "set -ArtifactUrl, -Artifact, -ReleaseBaseUrl, or use -FromSource"
        }
        $script:ArtifactUrl = ($ReleaseBaseUrl.TrimEnd("/") + "/" + $Version + "/" + (Get-ArchiveName -Target $Target))
    }

    $tmp = New-TempDir
    $leaf = Split-Path ([System.Uri]$script:ArtifactUrl).AbsolutePath -Leaf
    if ([string]::IsNullOrWhiteSpace($leaf)) {
        $leaf = "mayhem-artifact.zip"
    }
    $archive = Join-Path $tmp $leaf

    Write-Log "downloading $script:ArtifactUrl"
    Invoke-Download -Uri $script:ArtifactUrl -OutFile $archive

    if ([string]::IsNullOrWhiteSpace($Sha256)) {
        try {
            Invoke-Download -Uri ($script:ArtifactUrl + ".sha256") -OutFile ($archive + ".sha256")
            Write-Log "downloaded checksum sidecar"
        } catch {
            Remove-Item -Path ($archive + ".sha256") -Force -ErrorAction SilentlyContinue
        }
    }

    return $archive
}

function Expand-MayhemArchive {
    param(
        [string]$ArchivePath,
        [string]$Destination
    )

    if ($ArchivePath -like "*.zip") {
        Expand-Archive -Path $ArchivePath -DestinationPath $Destination -Force
        return
    }

    if ($ArchivePath -like "*.tar.gz" -or $ArchivePath -like "*.tgz") {
        if (-not (Test-Command "tar")) {
            Fail "tar is required for tar.gz artifacts"
        }
        & tar -xzf $ArchivePath -C $Destination
        if ($LASTEXITCODE -ne 0) {
            Fail "tar failed to extract $ArchivePath"
        }
        return
    }

    Fail "unsupported artifact format: $ArchivePath"
}

function Copy-ArtifactBins {
    param(
        [string]$PackageRoot,
        [string[]]$VerifiedFiles
    )

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

    foreach ($bin in $Bins) {
        $name = "$bin.exe"
        $relativePath = "bin/$name"
        if ($VerifiedFiles -notcontains $relativePath) {
            $relativePath = $name
            if ($VerifiedFiles -notcontains $relativePath) {
                Fail "SHA256SUMS does not verify binary: $name"
            }
        }

        $source = Join-Path $PackageRoot $relativePath
        if (-not (Test-Path -Path $source -PathType Leaf)) {
            Fail "artifact is missing verified binary: $relativePath"
        }

        Copy-Item -Path $source -Destination (Join-Path $InstallDir $name) -Force
    }
}

function Verify-ExtractedChecksums {
    param([string]$ExtractDir)

    $allSums = @(Get-ChildItem -Path $ExtractDir -Recurse -File -Filter "SHA256SUMS" | Sort-Object FullName)
    $sums = $allSums | Select-Object -First 1
    if (-not $sums) {
        Fail "artifact is missing SHA256SUMS"
    }
    if ($allSums.Count -ne 1) {
        Fail "artifact contains multiple SHA256SUMS files"
    }

    $verified = 0
    $verifiedFiles = @()
    foreach ($line in Get-Content -Path $sums.FullName) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }

        $match = [regex]::Match($line, "^\s*([0-9a-fA-F]{64})\s+(.+?)\s*$")
        if (-not $match.Success) {
            Fail "invalid SHA256SUMS entry: $line"
        }

        $expected = $match.Groups[1].Value.ToLowerInvariant()
        $relativePath = $match.Groups[2].Value.Trim()
        if ([string]::IsNullOrWhiteSpace($relativePath)) {
            Fail "invalid SHA256SUMS entry with empty path"
        }
        if ($relativePath -match "^[A-Za-z]:[\\/]") {
            Fail "unsafe SHA256SUMS path: $relativePath"
        }
        if ([System.IO.Path]::IsPathRooted($relativePath)) {
            Fail "unsafe SHA256SUMS path: $relativePath"
        }
        $parts = $relativePath -split "[\\/]"
        if ($parts -contains ".." -or $parts -contains "." -or $parts -contains "") {
            Fail "unsafe SHA256SUMS path: $relativePath"
        }
        $normalizedRelativePath = $parts -join "/"

        $target = Join-Path $sums.DirectoryName ($parts -join [System.IO.Path]::DirectorySeparatorChar)
        if (-not (Test-Path -Path $target -PathType Leaf)) {
            Fail "SHA256SUMS references missing file: $relativePath"
        }
        if ($verifiedFiles -contains $normalizedRelativePath) {
            Fail "SHA256SUMS contains duplicate path: $normalizedRelativePath"
        }
        $actual = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            Fail "checksum mismatch for packaged file $relativePath`: expected $expected, got $actual"
        }
        $verifiedFiles += $normalizedRelativePath
        $verified += 1
    }

    if ($verified -eq 0) {
        Fail "SHA256SUMS contains no files"
    }
    Write-Log "verified $verified packaged file checksum(s)"
    return [pscustomobject]@{
        Root = $sums.DirectoryName
        Files = $verifiedFiles
    }
}

function Install-FromArtifact {
    $target = Get-TargetTriple
    $archive = Get-ArtifactPath -Target $target
    if (-not (Test-Path $archive)) {
        Fail "artifact not found: $archive"
    }

    Verify-Archive -ArchivePath $archive
    $extractDir = New-TempDir
    Expand-MayhemArchive -ArchivePath $archive -Destination $extractDir
    $verifiedPackage = Verify-ExtractedChecksums -ExtractDir $extractDir
    Copy-ArtifactBins -PackageRoot $verifiedPackage.Root -VerifiedFiles $verifiedPackage.Files
}

function Install-FromSource {
    if (-not (Test-Path (Join-Path $SourceDir "Cargo.toml"))) {
        Fail "source dir does not contain Cargo.toml: $SourceDir"
    }
    if (-not (Test-Command "cargo")) {
        Fail "Rust/Cargo is required for -FromSource installs"
    }

    Write-Log "building release binaries from $SourceDir"
    Push-Location $SourceDir
    try {
        & cargo build --release --workspace --bins
        if ($LASTEXITCODE -ne 0) {
            Fail "cargo build failed"
        }
    } finally {
        Pop-Location
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    foreach ($bin in $Bins) {
        $src = Join-Path (Join-Path (Join-Path $SourceDir "target") "release") "$bin.exe"
        if (-not (Test-Path $src)) {
            Fail "missing built binary: $src"
        }
        Copy-Item -Path $src -Destination (Join-Path $InstallDir "$bin.exe") -Force
    }
}

function Ensure-Node {
    if ($SkipNode) {
        return
    }

    if (-not (Test-Command "node") -or -not (Test-Command "npm")) {
        Fail "Node.js with npm is required for Pear bootstrap; install Node.js 20+ or rerun with -SkipPear"
    }

    $nodeVersion = (& node --version)
    $npmVersion = (& npm --version)
    Write-Log "found $nodeVersion and npm $npmVersion"
}

function Ensure-Pear {
    if ($SkipPear) {
        Write-Log "skipping Pear bootstrap"
        return
    }

    if (Test-Command "pear") {
        Write-Log ("found Pear at " + (Get-Command pear).Source)
        & pear --help *> $null
        return
    }

    Ensure-Node
    New-Item -ItemType Directory -Path $NpmPrefix -Force | Out-Null
    Write-Log "installing Pear runtime with npm prefix $NpmPrefix"
    & npm install -g pear --prefix $NpmPrefix
    if ($LASTEXITCODE -ne 0) {
        Fail "npm failed to install Pear"
    }

    Add-PathEntry -Entry $NpmPrefix
    $npmBin = Join-Path $NpmPrefix "bin"
    if (Test-Path $npmBin) {
        Add-PathEntry -Entry $npmBin
    }

    if (-not (Test-Command "pear")) {
        Fail "Pear was installed but is not on PATH"
    }

    try {
        & pear --help *> $null
    } catch {
        Write-Warn "Pear installed; run 'pear' once if it asks to finish local setup"
    }
}

function Install-Opencode {
    if ($SkipOpencode) {
        Write-Log "skipping opencode install"
        return
    }

    if (-not $ForceOpencode -and (Test-Command "opencode")) {
        Write-Log ("found opencode at " + (Get-Command opencode).Source + "; skipping pinned install")
        return
    }

    $version = $OpencodeVersion.TrimStart("v")
    if ($version -ne "1.17.13") {
        Fail "opencode installer checksums are pinned for v1.17.13; got v$version"
    }

    $asset = Get-OpencodeAsset
    $url = "https://github.com/anomalyco/opencode/releases/download/v$version/$($asset.Name)"
    $tmp = New-TempDir
    $archive = Join-Path $tmp $asset.Name
    $extractDir = New-TempDir

    Write-Log "downloading opencode v$version ($($asset.Name))"
    Invoke-Download -Uri $url -OutFile $archive
    $actual = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash.ToLowerInvariant()
    if ($actual -ne $asset.Sha256) {
        Fail "opencode checksum mismatch for $($asset.Name): expected $($asset.Sha256), got $actual"
    }

    Expand-Archive -Path $archive -DestinationPath $extractDir -Force
    $matches = @(Get-ChildItem -Path $extractDir -Recurse -File -Filter "opencode.exe")
    if ($matches.Count -eq 0) {
        Fail "opencode archive did not contain opencode.exe"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -Path $matches[0].FullName -Destination (Join-Path $InstallDir "opencode.exe") -Force
    Write-Log "installed opencode v$version into $InstallDir"
}

function Update-UserPath {
    if ($script:PathEntries.Count -eq 0) {
        return
    }

    $joined = ($script:PathEntries -join ";")
    Write-Host ""
    Write-Host "Copy/paste PATH for this PowerShell session:"
    Write-Host ("  `$env:Path = `"{0};`$env:Path`"" -f $joined)

    if ($NoPathUpdate) {
        Write-Log "skipping user PATH update"
        return
    }

    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if (-not [string]::IsNullOrWhiteSpace($current)) {
        $parts = @($current -split ";")
    }

    foreach ($entry in $script:PathEntries) {
        if ($parts -notcontains $entry) {
            $parts += $entry
        }
    }

    [Environment]::SetEnvironmentVariable("Path", ($parts -join ";"), "User")
    Write-Log "updated user PATH; open a new terminal to pick it up"
}

function Smoke-Test {
    $mayhem = Join-Path $InstallDir "mayhem.exe"
    & $mayhem --help *> $null
    if ($LASTEXITCODE -ne 0) {
        Fail "installed mayhem binary did not run"
    }
    Write-Log "mayhem CLI smoke test passed"
}

function Main {
    if (-not $FromSource -and
        [string]::IsNullOrWhiteSpace($Artifact) -and
        [string]::IsNullOrWhiteSpace($ArtifactUrl) -and
        [string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
        if ((Test-Path (Join-Path $SourceDir "Cargo.toml")) -and (Test-Path (Join-Path (Join-Path $SourceDir "crates") "mayhem-cli"))) {
            $script:FromSource = $true
            $FromSource = $true
        }
    }

    Add-PathEntry -Entry $InstallDir
    Ensure-Pear

    if ($FromSource) {
        Install-FromSource
    } else {
        Install-FromArtifact
    }

    Install-Opencode
    Update-UserPath
    Smoke-Test
    Write-Log "installed Mayhem binaries into $InstallDir"
}

try {
    Main
} finally {
    foreach ($dir in $script:TempDirs) {
        Remove-Item -Path $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
