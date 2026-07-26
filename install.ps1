#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$FromSource,
    [string]$SourceDir = $(if ($env:MAYHEM_SOURCE_DIR) { $env:MAYHEM_SOURCE_DIR } else { $PSScriptRoot }),
    [string]$Artifact = $env:MAYHEM_ARTIFACT,
    [string]$ArtifactUrl = $env:MAYHEM_ARTIFACT_URL,
    [string]$Sha256 = $env:MAYHEM_ARTIFACT_SHA256,
    [string]$Manifest = $env:MAYHEM_RELEASE_MANIFEST,
    [string]$ManifestUrl = $env:MAYHEM_RELEASE_MANIFEST_URL,
    [string]$Signature = $env:MAYHEM_RELEASE_SIGNATURE,
    [string]$SignatureUrl = $env:MAYHEM_RELEASE_SIGNATURE_URL,
    [string]$ReleaseKey = $env:MAYHEM_RELEASE_KEY,
    [string]$ReleaseKeyId = $env:MAYHEM_RELEASE_KEY_ID,
    [string]$ExpectedSourceGitSha = $env:MAYHEM_SOURCE_GIT_SHA,
    [string]$ReleaseBaseUrl = $env:MAYHEM_RELEASE_BASE_URL,
    [string]$Version = $(if ($env:MAYHEM_VERSION) { $env:MAYHEM_VERSION } else { "latest" }),
    [string]$InstallDir = $(if ($env:MAYHEM_INSTALL_DIR) { $env:MAYHEM_INSTALL_DIR } else { Join-Path (Join-Path $HOME ".mayhem") "bin" }),
    [string]$ShareDir = $env:MAYHEM_SHARE_DIR,
    [switch]$SkipNode,
    [switch]$SkipPear,
    [switch]$SkipOpencode,
    [string]$OpencodeVersion = $(if ($env:MAYHEM_OPENCODE_VERSION) { $env:MAYHEM_OPENCODE_VERSION } else { "1.17.13" }),
    [switch]$ForceOpencode,
    [switch]$NoPathUpdate,
    [switch]$UnsignedLayout,
    [switch]$AllowUnverified,
    [string]$LlamaCppFeatures = $env:MAYHEM_LLAMA_CPP_FEATURES,
    [string]$NpmPrefix = $(if ($env:MAYHEM_NPM_PREFIX) { $env:MAYHEM_NPM_PREFIX } else { Join-Path (Join-Path $HOME ".mayhem") "node" })
)

$ErrorActionPreference = "Stop"
$script:VersionExplicit = $PSBoundParameters.ContainsKey("Version") -or
    $null -ne [Environment]::GetEnvironmentVariable("MAYHEM_VERSION", "Process")
$PearVersion = "2.0.4"
$script:ReleaseFloorPresent = $false

if ($env:MAYHEM_FROM_SOURCE -eq "1") { $FromSource = $true }
if ($env:MAYHEM_SKIP_NODE -eq "1") { $SkipNode = $true }
if ($env:MAYHEM_SKIP_PEAR -eq "1") { $SkipPear = $true }
if ($env:MAYHEM_SKIP_OPENCODE -eq "1") { $SkipOpencode = $true }
if ($env:MAYHEM_FORCE_OPENCODE -eq "1") { $ForceOpencode = $true }
if ($env:MAYHEM_NO_PATH_UPDATE -eq "1") { $NoPathUpdate = $true }
if ($env:MAYHEM_UNSIGNED_LAYOUT -eq "1") { $UnsignedLayout = $true }
if ($env:MAYHEM_ALLOW_UNVERIFIED -eq "1") { $AllowUnverified = $true }

$Bins = @(
    "mayhem",
    "mayhem-gateway",
    "mayhem-attestation-verifier",
    "mayhem-pay",
    "mayhemd",
    "mayhem-enclave",
    "mayhem-paygate"
)

$script:PathEntries = @()
$script:TempDirs = @()
$script:SourceLlamaCppBackend = ""

if ([string]::IsNullOrWhiteSpace($ShareDir)) {
    $ShareDir = Join-Path (Join-Path (Split-Path -Parent $InstallDir) "share") "mayhem"
}

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

function Assert-RealFile {
    param(
        [string]$Path,
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Fail "$Label is missing or not a regular file: $Path"
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        Fail "$Label must not be a reparse point: $Path"
    }
}

function Get-LlamaCppFeatureName {
    param([string]$Token)

    $normalized = $Token.Trim().ToLowerInvariant()
    switch ($normalized) {
        "cpu" { return "" }
        "none" { return "" }
        "cuda" { return "mayhem-cli/llama-cpp-cuda" }
        "llama-cpp-cuda" { return "mayhem-cli/llama-cpp-cuda" }
        "vulkan" { return "mayhem-cli/llama-cpp-vulkan" }
        "llama-cpp-vulkan" { return "mayhem-cli/llama-cpp-vulkan" }
        "openmp" { return "mayhem-cli/llama-cpp-openmp" }
        "llama-cpp-openmp" { return "mayhem-cli/llama-cpp-openmp" }
        "static-openmp" { return "mayhem-cli/llama-cpp-static-openmp" }
        "llama-cpp-static-openmp" { return "mayhem-cli/llama-cpp-static-openmp" }
        default {
            Fail "unknown MAYHEM_LLAMA_CPP_FEATURES entry '$Token' (expected cuda, vulkan, openmp, static-openmp, or cpu)"
        }
    }
}

function Test-CudaToolkitUsable {
    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace($env:CUDACXX)) {
        $candidates += $env:CUDACXX
    }
    foreach ($root in @($env:CUDA_PATH, $env:CUDA_HOME)) {
        if (-not [string]::IsNullOrWhiteSpace($root)) {
            $candidates += (Join-Path (Join-Path $root "bin") "nvcc.exe")
        }
    }
    $command = Get-Command "nvcc.exe" -CommandType Application -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        $candidates += $command.Path
    }

    foreach ($candidate in ($candidates | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            continue
        }
        try {
            & $candidate --version *> $null
            if ($LASTEXITCODE -eq 0) {
                return $true
            }
        } catch {
            continue
        }
    }
    return $false
}

function Test-VulkanToolkitUsable {
    if ([string]::IsNullOrWhiteSpace($env:VULKAN_SDK)) {
        return $false
    }
    $header = Join-Path (Join-Path (Join-Path $env:VULKAN_SDK "Include") "vulkan") "vulkan.h"
    $loader = Join-Path (Join-Path $env:VULKAN_SDK "Lib") "vulkan-1.lib"
    return (Test-Path -LiteralPath $header -PathType Leaf) -and
        (Test-Path -LiteralPath $loader -PathType Leaf)
}

function Assert-LlamaCppFeaturePrereqs {
    param(
        [string[]]$Features,
        [string]$TargetTriple = $(Get-TargetTriple)
    )

    $hasCuda = $Features -contains "mayhem-cli/llama-cpp-cuda"
    $hasVulkan = $Features -contains "mayhem-cli/llama-cpp-vulkan"
    if ($hasCuda -and $hasVulkan) {
        Fail "source build selected both CUDA and Vulkan; select exactly one llama.cpp backend"
    }
    if ($TargetTriple -eq "aarch64-pc-windows-msvc" -and ($hasCuda -or $hasVulkan)) {
        Fail "Windows ARM64 source builds support the llama.cpp CPU backend only; set MAYHEM_LLAMA_CPP_FEATURES=cpu"
    }
    if ($hasCuda -and -not (Test-CudaToolkitUsable)) {
        Fail "llama.cpp CUDA source build requested, but a working nvcc was not found; install CUDA Toolkit or set MAYHEM_LLAMA_CPP_FEATURES=cpu"
    }
    if ($hasVulkan -and -not (Test-VulkanToolkitUsable)) {
        Fail "llama.cpp Vulkan source build requested, but Vulkan headers and loader library were not found; install the Vulkan SDK or set MAYHEM_LLAMA_CPP_FEATURES=cpu"
    }
}

function Resolve-LlamaCppSourceBuild {
    param([string]$TargetTriple = $(Get-TargetTriple))

    $features = @()
    $backend = ""
    if (-not [string]::IsNullOrWhiteSpace($LlamaCppFeatures)) {
        foreach ($token in ($LlamaCppFeatures -split "[,; ]+")) {
            if ([string]::IsNullOrWhiteSpace($token)) {
                continue
            }
            $normalized = $token.Trim().ToLowerInvariant()
            $feature = Get-LlamaCppFeatureName -Token $token
            $candidate = ""
            switch ($feature) {
                "mayhem-cli/llama-cpp-cuda" { $candidate = "cuda" }
                "mayhem-cli/llama-cpp-vulkan" { $candidate = "vulkan" }
                default {
                    if ($normalized -in @("cpu", "none")) {
                        $candidate = "cpu"
                    }
                }
            }
            if (-not [string]::IsNullOrWhiteSpace($candidate)) {
                if (-not [string]::IsNullOrWhiteSpace($backend) -and $backend -ne $candidate) {
                    Fail "MAYHEM_LLAMA_CPP_FEATURES selects conflicting llama.cpp backends '$backend' and '$candidate'; select exactly one of cuda, vulkan, or cpu"
                }
                $backend = $candidate
            }
            if (-not [string]::IsNullOrWhiteSpace($feature) -and $features -notcontains $feature) {
                $features += $feature
            }
        }
        if ([string]::IsNullOrWhiteSpace($backend)) {
            $backend = "cpu"
        }
    } elseif ($TargetTriple -eq "aarch64-pc-windows-msvc") {
        $backend = "cpu"
    } elseif (Test-CudaToolkitUsable) {
        $backend = "cuda"
        $features += "mayhem-cli/llama-cpp-cuda"
    } elseif (Test-VulkanToolkitUsable) {
        $backend = "vulkan"
        $features += "mayhem-cli/llama-cpp-vulkan"
    } else {
        $backend = "cpu"
    }

    Assert-LlamaCppFeaturePrereqs -Features $features -TargetTriple $TargetTriple
    return [pscustomobject]@{
        Backend = $backend
        Features = [string[]]$features
    }
}

function Get-LlamaCppFeatures {
    param([string]$TargetTriple = $(Get-TargetTriple))

    $selection = Resolve-LlamaCppSourceBuild -TargetTriple $TargetTriple
    return @($selection.Features)
}

function Get-LlamaCppFeatureArgs {
    param(
        [string[]]$Features = $(Get-LlamaCppFeatures),
        [string]$TargetTriple = $(Get-TargetTriple)
    )

    Assert-LlamaCppFeaturePrereqs -Features $Features -TargetTriple $TargetTriple

    if ($Features.Count -eq 0) {
        Write-Log "building llama.cpp CPU fallback; set MAYHEM_LLAMA_CPP_FEATURES=cuda or vulkan for GPU source builds"
        return @()
    }

    Write-Log ("building llama.cpp provider feature(s): " + ($Features -join ", "))
    return @("--features", ($Features -join ","))
}

function Get-WindowsSourceBuildCargoArgs {
    param([string[]]$LlamaCppFeatures)

    if ($LlamaCppFeatures -contains "mayhem-cli/llama-cpp-vulkan") {
        return @("--jobs", "1")
    }
    return @()
}

function New-TempDir {
    param(
        [string]$BasePath = $([System.IO.Path]::GetTempPath()),
        [string]$Prefix = "mayhem-install-"
    )

    if ([string]::IsNullOrWhiteSpace($BasePath)) {
        Fail "could not determine a base directory for private installer state"
    }
    New-Item -ItemType Directory -Path $BasePath -Force | Out-Null
    $dir = Join-Path $BasePath ($Prefix + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $dir | Out-Null
    try {
        $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
        $acl = New-Object System.Security.AccessControl.DirectorySecurity
        $acl.SetOwner($identity.User)
        $acl.SetAccessRuleProtection($true, $false)
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
            [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
        $propagation = [System.Security.AccessControl.PropagationFlags]::None
        $allow = [System.Security.AccessControl.AccessControlType]::Allow
        $userRule = [System.Security.AccessControl.FileSystemAccessRule]::new(
            $identity.User,
            [System.Security.AccessControl.FileSystemRights]::FullControl,
            $inheritance,
            $propagation,
            $allow
        )
        $systemSid = [System.Security.Principal.SecurityIdentifier]::new(
            [System.Security.Principal.WellKnownSidType]::LocalSystemSid,
            $null
        )
        $systemRule = [System.Security.AccessControl.FileSystemAccessRule]::new(
            $systemSid,
            [System.Security.AccessControl.FileSystemRights]::FullControl,
            $inheritance,
            $propagation,
            $allow
        )
        $acl.AddAccessRule($userRule) | Out-Null
        $acl.AddAccessRule($systemRule) | Out-Null
        Set-Acl -LiteralPath $dir -AclObject $acl
    } catch {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
        Fail "could not create a private installer temp directory: $($_.Exception.Message)"
    }
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

function Get-WindowsSourceBuildArchitecture {
    param([string]$TargetTriple = $(Get-TargetTriple))

    switch ($TargetTriple) {
        "x86_64-pc-windows-msvc" {
            return [pscustomobject]@{
                TargetTriple = $TargetTriple
                VsTarget = "amd64"
                VsHost = "amd64"
                RequiredComponent = "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"
            }
        }
        "aarch64-pc-windows-msvc" {
            return [pscustomobject]@{
                TargetTriple = $TargetTriple
                VsTarget = "arm64"
                VsHost = "arm64"
                RequiredComponent = "Microsoft.VisualStudio.Component.VC.Tools.ARM64"
            }
        }
        default {
            Fail "unsupported Windows source-build target: $TargetTriple"
        }
    }
}

function Get-VsWherePath {
    $command = Get-Command "vswhere.exe" -CommandType Application -ErrorAction SilentlyContinue
    if ($null -ne $command -and (Test-Path -LiteralPath $command.Path -PathType Leaf)) {
        return $command.Path
    }

    $roots = @()
    foreach ($name in @("ProgramFiles(x86)", "ProgramFiles")) {
        $root = [Environment]::GetEnvironmentVariable($name, "Process")
        if (-not [string]::IsNullOrWhiteSpace($root) -and $roots -notcontains $root) {
            $roots += $root
        }
    }
    foreach ($root in $roots) {
        $candidate = Join-Path $root "Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }

    Fail "Visual Studio Locator (vswhere.exe) was not found; install Visual Studio Build Tools with the C++ tools required for this architecture"
}

function Find-VisualStudioSourceBuildTools {
    param(
        [pscustomobject]$Architecture,
        [string]$VsWherePath = $(Get-VsWherePath)
    )

    $arguments = @(
        "-latest",
        "-products",
        "*",
        "-requires",
        "Microsoft.Component.MSBuild",
        $Architecture.RequiredComponent,
        "-property",
        "installationPath"
    )
    $installPaths = @(& $VsWherePath @arguments)
    if ($LASTEXITCODE -ne 0) {
        Fail "vswhere.exe failed while locating Visual Studio C++ Build Tools"
    }
    $installPath = $installPaths |
        ForEach-Object { "$_".Trim() } |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($installPath)) {
        Fail "Visual Studio C++ Build Tools for $($Architecture.TargetTriple) were not found (missing $($Architecture.RequiredComponent))"
    }

    $vsDevCmd = Join-Path $installPath "Common7\Tools\VsDevCmd.bat"
    if (-not (Test-Path -LiteralPath $vsDevCmd -PathType Leaf)) {
        Fail "Visual Studio developer environment script was not found: $vsDevCmd"
    }
    return [pscustomobject]@{
        InstallationPath = $installPath
        VsDevCmd = $vsDevCmd
    }
}

function Get-VsDevCmdBatchContent {
    param([pscustomobject]$Architecture)

    return @(
        "@echo off",
        "call `"%_MAYHEM_VSDEVCMD_PATH%`" -no_logo -arch=$($Architecture.VsTarget) -host_arch=$($Architecture.VsHost)",
        "if errorlevel 1 exit /b %errorlevel%",
        "`"%_MAYHEM_POWERSHELL_EXE%`" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"%_MAYHEM_VSENV_DUMP_SCRIPT%`"",
        "exit /b %errorlevel%"
    ) -join "`r`n"
}

function Set-ProcessEnvironmentFromSnapshot {
    param([pscustomobject]$Snapshot)

    foreach ($property in $Snapshot.PSObject.Properties) {
        $name = [string]$property.Name
        if ($name.StartsWith("=") -or $name.StartsWith("_MAYHEM_VS")) {
            continue
        }
        [Environment]::SetEnvironmentVariable(
            $name,
            [string]$property.Value,
            "Process")
    }
}

function Import-VisualStudioDeveloperEnvironment {
    param(
        [string]$VsDevCmd,
        [pscustomobject]$Architecture
    )

    $tempDir = New-TempDir
    $batchPath = Join-Path $tempDir "initialize-vs.cmd"
    $dumpScript = Join-Path $tempDir "export-environment.ps1"
    $snapshotPath = Join-Path $tempDir "environment.json"
    $powerShellExe = if ($PSVersionTable.PSEdition -eq "Core") {
        Join-Path $PSHOME "pwsh.exe"
    } else {
        Join-Path $PSHOME "powershell.exe"
    }
    if (-not (Test-Path -LiteralPath $powerShellExe -PathType Leaf)) {
        Fail "could not locate the current PowerShell executable: $powerShellExe"
    }

    Set-Content -LiteralPath $batchPath `
        -Value (Get-VsDevCmdBatchContent -Architecture $Architecture) `
        -Encoding Ascii
    Set-Content -LiteralPath $dumpScript -Encoding UTF8 -Value @'
$ErrorActionPreference = "Stop"
$snapshot = [ordered]@{}
foreach ($entry in [Environment]::GetEnvironmentVariables("Process").GetEnumerator()) {
    $snapshot[[string]$entry.Key] = [string]$entry.Value
}
$snapshot |
    ConvertTo-Json -Compress |
    Set-Content -LiteralPath $env:_MAYHEM_VSENV_OUTPUT -Encoding UTF8
'@

    $helperVariables = @{
        "_MAYHEM_VSDEVCMD_PATH" = $VsDevCmd
        "_MAYHEM_POWERSHELL_EXE" = $powerShellExe
        "_MAYHEM_VSENV_DUMP_SCRIPT" = $dumpScript
        "_MAYHEM_VSENV_OUTPUT" = $snapshotPath
    }
    $previousValues = @{}
    foreach ($entry in $helperVariables.GetEnumerator()) {
        $previousValues[$entry.Key] = [Environment]::GetEnvironmentVariable(
            $entry.Key,
            "Process")
        [Environment]::SetEnvironmentVariable(
            $entry.Key,
            $entry.Value,
            "Process")
    }

    try {
        & $batchPath
        if ($LASTEXITCODE -ne 0) {
            Fail "Visual Studio developer environment initialization failed for $($Architecture.TargetTriple)"
        }
        if (-not (Test-Path -LiteralPath $snapshotPath -PathType Leaf)) {
            Fail "Visual Studio developer environment did not produce an environment snapshot"
        }
        try {
            $snapshot = Get-Content -LiteralPath $snapshotPath -Raw |
                ConvertFrom-Json
        } catch {
            Fail "Visual Studio developer environment snapshot is invalid: $($_.Exception.Message)"
        }
        Set-ProcessEnvironmentFromSnapshot -Snapshot $snapshot
    } finally {
        foreach ($entry in $previousValues.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable(
                $entry.Key,
                $entry.Value,
                "Process")
        }
    }
}

function Test-VsArchitectureValue {
    param(
        [string]$Actual,
        [string]$Expected
    )

    if ($Expected -eq "amd64") {
        return $Actual -in @("amd64", "x64")
    }
    return $Actual -eq $Expected
}

function Assert-WindowsSourceBuildEnvironment {
    param([pscustomobject]$Architecture)

    if (-not (Test-VsArchitectureValue -Actual $env:VSCMD_ARG_TGT_ARCH -Expected $Architecture.VsTarget)) {
        Fail "Visual Studio developer environment selected target '$env:VSCMD_ARG_TGT_ARCH' instead of '$($Architecture.VsTarget)'"
    }
    if (-not (Test-VsArchitectureValue -Actual $env:VSCMD_ARG_HOST_ARCH -Expected $Architecture.VsHost)) {
        Fail "Visual Studio developer environment selected host '$env:VSCMD_ARG_HOST_ARCH' instead of '$($Architecture.VsHost)'"
    }
    foreach ($name in @("VSINSTALLDIR", "VCINSTALLDIR", "VCToolsInstallDir", "WindowsSdkDir")) {
        if ([string]::IsNullOrWhiteSpace(
            [Environment]::GetEnvironmentVariable($name, "Process"))) {
            Fail "Visual Studio developer environment did not set $name"
        }
    }
    foreach ($command in @("cl.exe", "msbuild.exe", "cmake.exe")) {
        if (-not (Test-Command $command)) {
            Fail "Visual Studio developer environment did not expose $command"
        }
    }
}

function Initialize-WindowsSourceBuildEnvironment {
    param([string]$TargetTriple = $(Get-TargetTriple))

    $architecture = Get-WindowsSourceBuildArchitecture -TargetTriple $TargetTriple
    $tools = Find-VisualStudioSourceBuildTools -Architecture $architecture
    Import-VisualStudioDeveloperEnvironment `
        -VsDevCmd $tools.VsDevCmd `
        -Architecture $architecture
    Assert-WindowsSourceBuildEnvironment -Architecture $architecture
    Write-Log "initialized Visual Studio native build environment for $TargetTriple from $($tools.InstallationPath)"
}

function Get-WindowsLocalAppDataPath {
    if (-not ("MayhemWindowsUserProfile" -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class MayhemWindowsUserProfile {
    [DllImport(
        "userenv.dll",
        CharSet = CharSet.Unicode,
        ExactSpelling = true,
        SetLastError = true)]
    private static extern bool GetUserProfileDirectoryW(
        IntPtr token,
        StringBuilder profileDirectory,
        ref uint size);

    public static string GetPath(IntPtr token) {
        uint size = 0;
        GetUserProfileDirectoryW(token, null, ref size);
        if (size == 0) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        var profileDirectory = new StringBuilder((int)size);
        if (!GetUserProfileDirectoryW(token, profileDirectory, ref size)) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        return profileDirectory.ToString();
    }
}
"@ | Out-Null
    }

    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    try {
        $profilePath = [MayhemWindowsUserProfile]::GetPath($identity.Token)
    } finally {
        $identity.Dispose()
    }
    if ([string]::IsNullOrWhiteSpace($profilePath)) {
        Fail "could not determine the current Windows user's profile directory"
    }
    return Join-Path $profilePath "AppData\Local"
}

function New-WindowsSourceBuildTargetDir {
    $candidates = @()
    $localAppData = Get-WindowsLocalAppDataPath
    if (-not [string]::IsNullOrWhiteSpace($localAppData)) {
        $candidates += Join-Path $localAppData "Temp"
    }
    $processTemp = [IO.Path]::GetTempPath()
    if (-not [string]::IsNullOrWhiteSpace($processTemp) -and
        $candidates -notcontains $processTemp) {
        $candidates += $processTemp
    }

    $failures = @()
    foreach ($basePath in ($candidates | Sort-Object { $_.Length })) {
        try {
            return New-TempDir `
                -BasePath $basePath `
                -Prefix "mayhem-source-build-"
        } catch {
            $failures += $_.Exception.Message
        }
    }
    Fail "could not create a short private Windows source-build directory: $($failures -join '; ')"
}

function Get-ArchiveName {
    param([string]$Target)
    $artifactVersion = $Version -replace "^v", ""
    return "mayhem-$artifactVersion-$Target.zip"
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
        Fail "unsigned test layout is missing a checksum; pass -Sha256 or place a .sha256 sidecar next to it"
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

    if ($UnsignedLayout -and [string]::IsNullOrWhiteSpace($Sha256)) {
        try {
            Invoke-Download -Uri ($script:ArtifactUrl + ".sha256") -OutFile ($archive + ".sha256")
            Write-Log "downloaded checksum sidecar"
        } catch {
            Remove-Item -Path ($archive + ".sha256") -Force -ErrorAction SilentlyContinue
        }
    }

    return $archive
}

function Get-ReleaseArtifactStem {
    param([string]$Value)

    if ($Value.EndsWith(".tar.gz", [StringComparison]::OrdinalIgnoreCase)) {
        return $Value.Substring(0, $Value.Length - ".tar.gz".Length)
    }
    if ($Value.EndsWith(".tgz", [StringComparison]::OrdinalIgnoreCase)) {
        return $Value.Substring(0, $Value.Length - ".tgz".Length)
    }
    if ($Value.EndsWith(".zip", [StringComparison]::OrdinalIgnoreCase)) {
        return $Value.Substring(0, $Value.Length - ".zip".Length)
    }
    Fail "signed release archive must end in .tar.gz, .tgz, or .zip: $Value"
}

function Assert-SignedReleaseSelection {
    if (-not [string]::IsNullOrWhiteSpace($ExpectedSourceGitSha) -and
        $ExpectedSourceGitSha -cnotmatch "^[0-9a-f]{40}$") {
        Fail "-ExpectedSourceGitSha must be exactly 40 lowercase hexadecimal characters"
    }
    if ($Version -ceq "latest") {
        if ([string]::IsNullOrWhiteSpace($ExpectedSourceGitSha)) {
            Fail "signed installs cannot use unpinned latest; pass an exact -Version or -ExpectedSourceGitSha"
        }
        return
    }
    if ($Version -cnotmatch "^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$") {
        Fail "-Version must be an exact canonical semantic version for signed installs"
    }
    if (-not $script:VersionExplicit) {
        Fail "signed installs require an explicitly requested version or source_git_sha"
    }
}

function Resolve-SignedReleaseMetadata {
    param([string]$ArchivePath)

    $tmp = New-TempDir
    if ([string]::IsNullOrWhiteSpace($script:Manifest)) {
        if (-not [string]::IsNullOrWhiteSpace($script:ManifestUrl)) {
            $script:Manifest = Join-Path $tmp "manifest.json"
            Write-Log "downloading $script:ManifestUrl"
            Invoke-Download -Uri $script:ManifestUrl -OutFile $script:Manifest
        } elseif (-not [string]::IsNullOrWhiteSpace($script:ArtifactUrl)) {
            $remotePath = ([System.Uri]$script:ArtifactUrl).GetLeftPart([System.UriPartial]::Path)
            $script:ManifestUrl = (Get-ReleaseArtifactStem -Value $remotePath) + ".manifest.json"
            $script:Manifest = Join-Path $tmp "manifest.json"
            Write-Log "downloading $script:ManifestUrl"
            Invoke-Download -Uri $script:ManifestUrl -OutFile $script:Manifest
        } else {
            $script:Manifest = (Get-ReleaseArtifactStem -Value $ArchivePath) + ".manifest.json"
        }
    }
    if ([string]::IsNullOrWhiteSpace($script:Signature)) {
        if (-not [string]::IsNullOrWhiteSpace($script:SignatureUrl)) {
            $script:Signature = Join-Path $tmp "manifest.json.sig"
            Write-Log "downloading $script:SignatureUrl"
            Invoke-Download -Uri $script:SignatureUrl -OutFile $script:Signature
        } elseif (-not [string]::IsNullOrWhiteSpace($script:ArtifactUrl)) {
            $remotePath = ([System.Uri]$script:ArtifactUrl).GetLeftPart([System.UriPartial]::Path)
            $script:SignatureUrl = (Get-ReleaseArtifactStem -Value $remotePath) + ".manifest.json.sig"
            $script:Signature = Join-Path $tmp "manifest.json.sig"
            Write-Log "downloading $script:SignatureUrl"
            Invoke-Download -Uri $script:SignatureUrl -OutFile $script:Signature
        } else {
            $script:Signature = (Get-ReleaseArtifactStem -Value $ArchivePath) + ".manifest.json.sig"
        }
    }
    if ([string]::IsNullOrWhiteSpace($ReleaseKey)) {
        Fail "signed installs require -ReleaseKey with an independently trusted public key record"
    }
}

function Initialize-ReleaseSnapshotCopy {
    if ("MayhemReleaseSnapshot" -as [type]) {
        return
    }
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class MayhemReleaseSnapshot {
    private const uint GENERIC_READ = 0x80000000;
    private const uint FILE_SHARE_READ = 0x00000001;
    private const uint OPEN_EXISTING = 3;
    private const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
    private const uint FILE_FLAG_SEQUENTIAL_SCAN = 0x08000000;
    private const uint FILE_ATTRIBUTE_DIRECTORY = 0x00000010;
    private const uint FILE_ATTRIBUTE_REPARSE_POINT = 0x00000400;
    private const uint FILE_TYPE_DISK = 0x0001;

    [StructLayout(LayoutKind.Sequential)]
    private struct ByHandleFileInformation {
        public uint FileAttributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFile(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(
        SafeFileHandle handle,
        out ByHandleFileInformation information);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint GetFileType(SafeFileHandle handle);

    public static void CopyBoundedRegularFile(
        string source,
        string destination,
        long maximum,
        string label) {
        SafeFileHandle handle = CreateFile(
            source,
            GENERIC_READ,
            FILE_SHARE_READ,
            IntPtr.Zero,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN,
            IntPtr.Zero);
        if (handle.IsInvalid) {
            throw new Win32Exception(
                Marshal.GetLastWin32Error(),
                label + " could not be opened for a private snapshot");
        }
        try {
            ByHandleFileInformation information;
            if (!GetFileInformationByHandle(handle, out information)) {
                throw new Win32Exception(
                    Marshal.GetLastWin32Error(),
                    label + " could not be inspected");
            }
            if (GetFileType(handle) != FILE_TYPE_DISK ||
                (information.FileAttributes &
                    (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)) != 0) {
                throw new InvalidDataException(
                    label + " must be a regular non-reparse-point disk file");
            }
            ulong unsignedLength =
                ((ulong)information.FileSizeHigh << 32) | information.FileSizeLow;
            if (unsignedLength == 0 || unsignedLength > (ulong)maximum) {
                throw new InvalidDataException(label + " exceeds its bounded size");
            }
            long expectedLength = checked((long)unsignedLength);
            using (FileStream input = new FileStream(handle, FileAccess.Read, 131072, false))
            using (FileStream output = new FileStream(
                destination,
                FileMode.CreateNew,
                FileAccess.Write,
                FileShare.None,
                131072,
                FileOptions.WriteThrough)) {
                byte[] buffer = new byte[131072];
                long total = 0;
                while (true) {
                    int read = input.Read(buffer, 0, buffer.Length);
                    if (read == 0) {
                        break;
                    }
                    total = checked(total + read);
                    if (total > maximum) {
                        throw new InvalidDataException(label + " exceeded its snapshot limit");
                    }
                    output.Write(buffer, 0, read);
                }
                if (total != expectedLength) {
                    throw new InvalidDataException(label + " changed while it was snapshotted");
                }
                output.Flush(true);
            }
        } finally {
            handle.Dispose();
        }
    }
}
'@
}

function Snapshot-SignedReleaseInputs {
    param([string]$ArchivePath)

    if (-not $ArchivePath.EndsWith(".zip", [StringComparison]::OrdinalIgnoreCase)) {
        Fail "install.ps1 signed releases require a canonical .zip archive"
    }
    Initialize-ReleaseSnapshotCopy
    $snapshotRoot = New-TempDir
    $archiveSnapshot = Join-Path $snapshotRoot "release.zip"
    $manifestSnapshot = Join-Path $snapshotRoot "manifest.json"
    $signatureSnapshot = Join-Path $snapshotRoot "manifest.json.sig"
    $keySnapshot = Join-Path $snapshotRoot "release-key.json"
    try {
        [MayhemReleaseSnapshot]::CopyBoundedRegularFile(
            $ArchivePath, $archiveSnapshot, 2147483648, "release archive")
        [MayhemReleaseSnapshot]::CopyBoundedRegularFile(
            $script:Manifest, $manifestSnapshot, 67108864, "release manifest")
        [MayhemReleaseSnapshot]::CopyBoundedRegularFile(
            $script:Signature, $signatureSnapshot, 65536, "release signature")
        [MayhemReleaseSnapshot]::CopyBoundedRegularFile(
            $ReleaseKey, $keySnapshot, 65536, "trusted release key")
    } catch {
        Fail "could not snapshot signed release inputs: $($_.Exception.Message)"
    }
    $script:Manifest = $manifestSnapshot
    $script:Signature = $signatureSnapshot
    $script:ReleaseKey = $keySnapshot
    Write-Log "snapshotted signed release inputs into private installer state"
    return $archiveSnapshot
}

function Get-SignedInstallState {
    $installRoot = Split-Path -Parent $InstallDir
    $expectedBin = Join-Path $installRoot "bin"
    $expectedShare = Join-Path (Join-Path $installRoot "share") "mayhem"
    if (-not [StringComparer]::OrdinalIgnoreCase.Equals(
        [IO.Path]::GetFullPath($InstallDir),
        [IO.Path]::GetFullPath($expectedBin))) {
        Fail "signed installs require the binary directory to be <install-root>\bin"
    }
    if (-not [StringComparer]::OrdinalIgnoreCase.Equals(
        [IO.Path]::GetFullPath($ShareDir),
        [IO.Path]::GetFullPath($expectedShare))) {
        Fail "signed installs require the asset directory to be <install-root>\share\mayhem"
    }
    if (Test-Path -LiteralPath $installRoot) {
        $installItem = Get-Item -LiteralPath $installRoot -Force
        if (-not $installItem.PSIsContainer -or
            ($installItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Fail "release install root must be a real directory: $installRoot"
        }
    }
    $updateRoot = Join-Path $installRoot ".mayhem-update"
    if (Test-Path -LiteralPath $updateRoot) {
        $updateItem = Get-Item -LiteralPath $updateRoot -Force
        if (-not $updateItem.PSIsContainer -or
            ($updateItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Fail "release update root must be a real directory: $updateRoot"
        }
    }
    $floor = Join-Path $updateRoot "release-floor.json"
    $script:ReleaseFloorPresent = Test-Path -LiteralPath $floor
    if ($script:ReleaseFloorPresent) {
        Assert-RealFile -Path $floor -Label "release anti-rollback floor"
    }
    return [pscustomobject]@{
        InstallRoot = $installRoot
        Floor = $floor
    }
}

function Get-BootstrapReleaseIdentity {
    param(
        [string]$Target,
        [string]$FloorPath
    )

    if (-not (Test-Command "node")) {
        Fail "Node.js is required to authenticate a signed release bootstrap"
    }
    $env:RELEASE_BOOTSTRAP_MANIFEST = $script:Manifest
    $env:RELEASE_BOOTSTRAP_SIGNATURE = $script:Signature
    $env:RELEASE_BOOTSTRAP_KEY = $ReleaseKey
    $env:RELEASE_BOOTSTRAP_TARGET = $Target
    $env:RELEASE_BOOTSTRAP_KEY_ID = $ReleaseKeyId
    $env:RELEASE_BOOTSTRAP_SOURCE_GIT_SHA = $ExpectedSourceGitSha
    $env:RELEASE_BOOTSTRAP_REQUESTED_VERSION = $(if ($Version -ceq "latest") { "" } else { $Version })
    $env:RELEASE_BOOTSTRAP_FLOOR = $FloorPath
    $env:RELEASE_BOOTSTRAP_FLOOR_PRESENT = $(if ($script:ReleaseFloorPresent) { "1" } else { "0" })
    $program = @'
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const readBounded = (input, maximum, label) => {
  const resolved = path.resolve(input);
  const stat = fs.lstatSync(resolved);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size === 0 || stat.size > maximum) {
    throw new Error(`${label} must be a bounded regular non-symlink file`);
  }
  return fs.readFileSync(resolved);
};
const manifestBytes = readBounded(
  process.env.RELEASE_BOOTSTRAP_MANIFEST, 64 * 1024 * 1024, "release manifest");
const signatureBytes = readBounded(
  process.env.RELEASE_BOOTSTRAP_SIGNATURE, 64 * 1024, "release signature");
const keyBytes = readBounded(
  process.env.RELEASE_BOOTSTRAP_KEY, 64 * 1024, "trusted release key");
const parse = (bytes, label) => {
  try {
    return JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    throw new Error(`${label} is invalid JSON: ${error.message}`);
  }
};
const manifest = parse(manifestBytes, "release manifest");
const signature = parse(signatureBytes, "release signature");
const key = parse(keyBytes, "trusted release key");
const exactKeys = (value, expected) =>
  value !== null &&
  typeof value === "object" &&
  !Array.isArray(value) &&
  JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...expected].sort());
if (!exactKeys(signature, [
  "schema_version", "alg", "signed_path", "key_id",
  "public_key", "sha256", "sig",
])) {
  throw new Error("release signature has an unexpected schema");
}
if (!exactKeys(key, ["key_id", "alg", "public_key", "status", "created_at"])) {
  throw new Error("trusted release key has an unexpected schema");
}
const validKeyId = (value) =>
  typeof value === "string" &&
  value.length > 0 &&
  value.length <= 128 &&
  /^[A-Za-z0-9._-]+$/.test(value);
if (!validKeyId(key.key_id) ||
    key.alg !== "ed25519" ||
    key.status !== "active" ||
    !/^[0-9a-f]{64}$/.test(key.public_key) ||
    signature.schema_version !== 1 ||
    signature.alg !== "ed25519" ||
    signature.key_id !== key.key_id ||
    signature.public_key !== key.public_key ||
    !/^[0-9a-f]{64}$/.test(signature.sha256) ||
    !/^[0-9a-f]{128}$/.test(signature.sig)) {
  throw new Error("release signature does not match the trusted active Ed25519 key");
}
const expectedKeyId = process.env.RELEASE_BOOTSTRAP_KEY_ID;
if (expectedKeyId && signature.key_id !== expectedKeyId) {
  throw new Error(`release signature key id ${signature.key_id} does not match ${expectedKeyId}`);
}
if (manifest?.schema !== 1 ||
    manifest.name !== "mayhem" ||
    !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(manifest.version) ||
    manifest.target !== process.env.RELEASE_BOOTSTRAP_TARGET ||
    !/^[0-9a-f]{40}$/.test(manifest.source_git_sha)) {
  throw new Error("signed release identity, target, version, or source_git_sha is invalid");
}
const expectedSource = process.env.RELEASE_BOOTSTRAP_SOURCE_GIT_SHA;
if (expectedSource && !/^[0-9a-f]{40}$/.test(expectedSource)) {
  throw new Error("expected source_git_sha must be exactly 40 lowercase hexadecimal characters");
}
if (expectedSource && manifest.source_git_sha !== expectedSource) {
  throw new Error(
    `signed release source_git_sha ${manifest.source_git_sha} does not match ${expectedSource}`);
}
const expectedSignedPath = `mayhem-${manifest.version}-${manifest.target}.manifest.json`;
if (signature.signed_path !== expectedSignedPath) {
  throw new Error("release signature signed_path does not match the release identity");
}
const digest = crypto.createHash("sha256").update(manifestBytes).digest("hex");
if (signature.sha256 !== digest) {
  throw new Error("release signature manifest hash does not match");
}
const publicKey = crypto.createPublicKey({
  key: Buffer.concat([
    Buffer.from("302a300506032b6570032100", "hex"),
    Buffer.from(key.public_key, "hex"),
  ]),
  format: "der",
  type: "spki",
});
const signingBytes = Buffer.concat([
  Buffer.from("mayhem.release-manifest.v1\n", "ascii"),
  manifestBytes,
]);
if (!crypto.verify(null, signingBytes, publicKey, Buffer.from(signature.sig, "hex"))) {
  throw new Error("release manifest Ed25519 signature verification failed");
}
const requestedVersion = process.env.RELEASE_BOOTSTRAP_REQUESTED_VERSION;
if (requestedVersion &&
    (!/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(requestedVersion) ||
     manifest.version !== requestedVersion)) {
  throw new Error(
    `signed release version ${manifest.version} does not match requested version ${requestedVersion}`);
}
if (!requestedVersion && !expectedSource) {
  throw new Error("signed release is not bound to an exact version or source_git_sha");
}
if (process.env.RELEASE_BOOTSTRAP_FLOOR_PRESENT === "1") {
  const floor = parse(
    readBounded(process.env.RELEASE_BOOTSTRAP_FLOOR, 4 * 1024, "release anti-rollback floor"),
    "release anti-rollback floor");
  if (!exactKeys(floor, ["schema", "version"]) ||
      floor.schema !== 1 ||
      !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(floor.version)) {
    throw new Error("release anti-rollback floor has an invalid schema or version");
  }
  const compareSemver = (left, right) => {
    const leftParts = left.split(".").map(BigInt);
    const rightParts = right.split(".").map(BigInt);
    for (let index = 0; index < 3; index += 1) {
      if (leftParts[index] < rightParts[index]) return -1;
      if (leftParts[index] > rightParts[index]) return 1;
    }
    return 0;
  };
  if (compareSemver(manifest.version, floor.version) < 0) {
    throw new Error(
      `signed release version ${manifest.version} is below protected anti-rollback floor ${floor.version}`);
  }
}
const binaryName = manifest.target.includes("windows") ? "mayhem.exe" : "mayhem";
const binaryPath = `bin/${binaryName}`;
const binaries = Array.isArray(manifest.binaries)
  ? manifest.binaries.filter((binary) =>
      binary?.name === binaryName && binary?.path === binaryPath)
  : [];
const assets = Array.isArray(manifest.assets)
  ? manifest.assets.filter((asset) => asset?.path === binaryPath)
  : [];
if (binaries.length !== 1 ||
    assets.length !== 1 ||
    !/^[0-9a-f]{64}$/.test(binaries[0].sha256) ||
    binaries[0].sha256 !== assets[0].sha256) {
  throw new Error("signed manifest does not bind exactly one bootstrap mayhem binary");
}
process.stdout.write([
  manifest.version,
  manifest.source_git_sha,
  signature.key_id,
  binaries[0].sha256,
  binaryPath,
].join("\t") + "\n");
'@
    $identity = & node "--input-type=module" "-e" $program
    if ($LASTEXITCODE -ne 0) {
        Fail "signed release bootstrap authentication failed"
    }
    return $identity
}

function Expand-AuthenticatedBootstrap {
    param(
        [string]$ArchivePath,
        [string]$Target,
        [string]$ReleaseVersion,
        [string]$BinaryPath,
        [string]$ExpectedSha256,
        [string]$Output
    )

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $entryName = "mayhem-$ReleaseVersion-$Target/$BinaryPath"
    $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        if ($archive.Entries.Count -gt 250000) {
            Fail "release ZIP exceeds the bounded entry count"
        }
        $seen = [System.Collections.Generic.HashSet[string]]::new(
            [StringComparer]::Ordinal)
        $seenPortable = [System.Collections.Generic.HashSet[string]]::new(
            [StringComparer]::OrdinalIgnoreCase)
        $entries = @()
        [UInt64]$totalLength = 0
        foreach ($entry in $archive.Entries) {
            $rawName = $entry.FullName
            $nameBytes = [Text.Encoding]::UTF8.GetByteCount($rawName)
            $isDirectory = $rawName.EndsWith("/", [StringComparison]::Ordinal)
            $normalizedName = $(if ($isDirectory) {
                $rawName.Substring(0, $rawName.Length - 1)
            } else {
                $rawName
            })
            if ($nameBytes -eq 0 -or $nameBytes -gt 1024 -or
                $normalizedName.Contains("\") -or
                $normalizedName.StartsWith("/", [StringComparison]::Ordinal) -or
                $normalizedName -cmatch '[<>:"|?*]' -or
                [IO.Path]::IsPathRooted($normalizedName)) {
                Fail "release ZIP contains an unsafe entry path: $rawName"
            }
            $parts = @($normalizedName.Split([char]"/"))
            if ($parts.Count -eq 0 -or
                @($parts | Where-Object {
                    [string]::IsNullOrEmpty($_) -or $_ -ceq "." -or $_ -ceq ".." -or
                    $_.EndsWith(".", [StringComparison]::Ordinal) -or
                    $_.EndsWith(" ", [StringComparison]::Ordinal)
                }).Count -ne 0) {
                Fail "release ZIP contains traversal or a non-portable entry path: $rawName"
            }
            if (-not $seen.Add($normalizedName) -or
                -not $seenPortable.Add($normalizedName)) {
                Fail "release ZIP contains a duplicate or case-colliding entry: $rawName"
            }
            $external = [BitConverter]::ToUInt32(
                [BitConverter]::GetBytes([Int32]$entry.ExternalAttributes),
                0
            )
            $unixType = ($external -shr 16) -band 0xF000
            $dosDirectory = ($external -band 0x10) -ne 0
            if ($unixType -notin @(0, 0x4000, 0x8000) -or
                ($unixType -eq 0x4000 -and -not $isDirectory) -or
                ($unixType -eq 0x8000 -and $isDirectory) -or
                ($dosDirectory -ne $isDirectory)) {
                Fail "release ZIP contains a link, special file, or ambiguous file type: $rawName"
            }
            if (-not $isDirectory) {
                if ($entry.Length -lt 0 -or $entry.Length -gt 2147483648) {
                    Fail "release ZIP contains an oversized file: $rawName"
                }
                $totalLength += [UInt64]$entry.Length
                if ($totalLength -gt 8589934592) {
                    Fail "release ZIP exceeds its total expanded size limit"
                }
            }
            if ($rawName -ceq $entryName) {
                $entries += $entry
            }
        }
        if ($entries.Count -ne 1 -or $entries[0].Length -le 0 -or $entries[0].Length -gt 2147483648) {
            Fail "release archive does not contain exactly one bounded signed bootstrap binary"
        }
        $input = $entries[0].Open()
        $outputStream = [System.IO.File]::Open(
            $Output,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        try {
            $input.CopyTo($outputStream)
        } finally {
            $outputStream.Dispose()
            $input.Dispose()
        }
    } finally {
        $archive.Dispose()
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Output).Hash.ToLowerInvariant()
    if ($actual -cne $ExpectedSha256) {
        Fail "signed bootstrap binary hash mismatch: expected $ExpectedSha256, got $actual"
    }
    Write-Log "authenticated bootstrap mayhem binary $actual"
}

function Install-TrustedReleaseKey {
    param(
        [string]$InstallRoot,
        [string]$KeyId
    )

    if (Test-Path -LiteralPath $InstallRoot) {
        $installItem = Get-Item -LiteralPath $InstallRoot -Force
        if (-not $installItem.PSIsContainer -or
            ($installItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Fail "release install root must be a real directory: $InstallRoot"
        }
    } else {
        New-Item -ItemType Directory -Path $InstallRoot | Out-Null
    }
    $updateRoot = Join-Path $InstallRoot ".mayhem-update"
    foreach ($directory in @($updateRoot, (Join-Path $updateRoot "trusted-release-keys"))) {
        if (Test-Path -LiteralPath $directory) {
            $item = Get-Item -LiteralPath $directory -Force
            if (-not $item.PSIsContainer -or
                ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                Fail "release trust path must be a real directory: $directory"
            }
        } else {
            New-Item -ItemType Directory -Path $directory | Out-Null
        }
    }
    $trustedRoot = Join-Path $updateRoot "trusted-release-keys"
    $destination = Join-Path $trustedRoot "$KeyId.json"
    if (Test-Path -LiteralPath $destination) {
        Assert-RealFile -Path $destination -Label "provisioned release key"
        $left = (Get-FileHash -Algorithm SHA256 -LiteralPath $ReleaseKey).Hash
        $right = (Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash
        if ($left -cne $right) {
            Fail "provisioned release key $KeyId differs from the trusted install state"
        }
    } else {
        Copy-Item -LiteralPath $ReleaseKey -Destination $destination
    }
    return $trustedRoot
}

function Install-SignedRelease {
    param(
        [string]$ArchivePath,
        [string]$Target
    )

    $state = Get-SignedInstallState
    $ArchivePath = Snapshot-SignedReleaseInputs -ArchivePath $ArchivePath
    $identity = Get-BootstrapReleaseIdentity -Target $Target -FloorPath $state.Floor
    $parts = $identity -split "`t"
    if ($parts.Count -ne 5) {
        Fail "signed release bootstrap identity was incomplete"
    }
    $releaseVersion = $parts[0]
    $sourceGitSha = $parts[1]
    $keyId = $parts[2]
    $bootstrapSha = $parts[3]
    $binaryPath = $parts[4]
    if (-not [string]::IsNullOrWhiteSpace($Sha256)) {
        Verify-Archive -ArchivePath $ArchivePath
    }

    $bootstrapRoot = New-TempDir
    $bootstrap = Join-Path $bootstrapRoot "mayhem.exe"
    Expand-AuthenticatedBootstrap `
        -ArchivePath $ArchivePath `
        -Target $Target `
        -ReleaseVersion $releaseVersion `
        -BinaryPath $binaryPath `
        -ExpectedSha256 $bootstrapSha `
        -Output $bootstrap
    $reportedVersion = & $bootstrap "--version"
    if ($LASTEXITCODE -ne 0 -or $reportedVersion -cne "mayhem $releaseVersion") {
        Fail "authenticated bootstrap binary version does not match signed release $releaseVersion"
    }

    $installRoot = $state.InstallRoot
    $trustedKeys = Install-TrustedReleaseKey -InstallRoot $installRoot -KeyId $keyId

    Write-Log "staging and reauthenticating signed release $releaseVersion ($sourceGitSha)"
    $stageReport = Join-Path $bootstrapRoot "stage.json"
    & $bootstrap update `
        --home $installRoot `
        --target $Target `
        --archive-path $ArchivePath `
        --manifest-path $script:Manifest `
        --signature-path $script:Signature `
        --release-keys-dir $trustedKeys `
        --key-id $keyId `
        --json *> $stageReport
    if ($LASTEXITCODE -ne 0) {
        Fail "signed release staging or reauthentication failed"
    }

    Write-Log "activating signed release $releaseVersion"
    $applyReport = Join-Path $bootstrapRoot "apply.json"
    & $bootstrap update `
        --home $installRoot `
        --target $Target `
        --apply-staged `
        --release-keys-dir $trustedKeys `
        --key-id $keyId `
        --bypass-apply-delay `
        --post-upgrade-arg=--help `
        --json *> $applyReport
    if ($LASTEXITCODE -ne 0) {
        Fail "signed release activation failed"
    }
    $floor = Join-Path (Join-Path $installRoot ".mayhem-update") "release-floor.json"
    if (-not (Test-Path -LiteralPath $floor -PathType Leaf)) {
        Fail "signed install did not provision the anti-rollback floor"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $trustedKeys "$KeyId.json") -PathType Leaf)) {
        Fail "signed install did not provision updater release trust"
    }
    Write-Log "verified and activated signed release $releaseVersion from $sourceGitSha"
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

function Join-RelativePath {
    param(
        [string]$Root,
        [string]$RelativePath
    )

    $path = $Root
    foreach ($part in ($RelativePath -split "/")) {
        if (-not [string]::IsNullOrWhiteSpace($part)) {
            $path = Join-Path $path $part
        }
    }
    return $path
}

function Reset-RuntimeAssetDir {
    $paths = @(
        (Join-Path $ShareDir "RULES.md"),
        (Join-Path $ShareDir "catalog"),
        (Join-Path $ShareDir "intercom"),
        (Join-Path $ShareDir "contracts"),
        (Join-Path (Join-Path (Join-Path $ShareDir "crates") "mayhem-cli") "src")
    )
    foreach ($path in $paths) {
        Remove-Item -Path $path -Recurse -Force -ErrorAction SilentlyContinue
    }
    New-Item -ItemType Directory -Path $ShareDir -Force | Out-Null
}

function Copy-FilteredDirectory {
    param(
        [string]$Source,
        [string]$Destination
    )

    if (-not (Test-Path -Path $Source -PathType Container)) {
        Fail "missing runtime asset directory: $Source"
    }
    $skip = @("node_modules", ".git", "tests", "test", "coverage", ".cache", "logs", "store")
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    foreach ($item in Get-ChildItem -LiteralPath $Source -Force) {
        if ($item.PSIsContainer) {
            if ($skip -contains $item.Name) {
                continue
            }
            Copy-FilteredDirectory -Source $item.FullName -Destination (Join-Path $Destination $item.Name)
        } else {
            Copy-Item -LiteralPath $item.FullName -Destination (Join-Path $Destination $item.Name) -Force
        }
    }
}

function Copy-SourceAssets {
    Reset-RuntimeAssetDir
    Copy-Item -Path (Join-Path $SourceDir "RULES.md") -Destination (Join-Path $ShareDir "RULES.md") -Force
    Copy-FilteredDirectory -Source (Join-Path $SourceDir "catalog") -Destination (Join-Path $ShareDir "catalog")
    Copy-FilteredDirectory -Source (Join-Path $SourceDir "intercom") -Destination (Join-Path $ShareDir "intercom")
    Copy-FilteredDirectory -Source (Join-Path $SourceDir "contracts") -Destination (Join-Path $ShareDir "contracts")
    $helperDir = Join-Path (Join-Path (Join-Path $ShareDir "crates") "mayhem-cli") "src"
    New-Item -ItemType Directory -Path $helperDir -Force | Out-Null
    foreach ($helper in Get-ChildItem -Path (Join-Path (Join-Path (Join-Path $SourceDir "crates") "mayhem-cli") "src") -Filter "*.mjs") {
        Copy-Item -LiteralPath $helper.FullName -Destination (Join-Path $helperDir $helper.Name) -Force
    }
    Write-Log "installed Mayhem runtime assets into $ShareDir"
}

function Copy-PackageAssets {
    param(
        [string]$PackageRoot,
        [string[]]$VerifiedFiles
    )

    if ($VerifiedFiles -notcontains "share/mayhem/RULES.md") {
        Fail "SHA256SUMS does not verify share/mayhem/RULES.md"
    }

    Reset-RuntimeAssetDir
    foreach ($relativePath in $VerifiedFiles) {
        if (-not $relativePath.StartsWith("share/mayhem/")) {
            continue
        }
        $assetRelative = $relativePath.Substring("share/mayhem/".Length)
        $source = Join-RelativePath -Root $PackageRoot -RelativePath $relativePath
        $target = Join-RelativePath -Root $ShareDir -RelativePath $assetRelative
        New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
        Copy-Item -LiteralPath $source -Destination $target -Force
    }
    Write-Log "installed Mayhem runtime assets into $ShareDir"
}

function Invoke-NpmInstall {
    param([string]$Directory)

    if (-not (Test-Path -Path (Join-Path $Directory "package.json") -PathType Leaf)) {
        return
    }
    Write-Log "installing runtime dependencies in $Directory"
    Push-Location $Directory
    try {
        if (Test-Path -Path "package-lock.json" -PathType Leaf) {
            & npm "ci" "--omit=dev"
        } else {
            & npm "install" "--omit=dev"
        }
        if ($LASTEXITCODE -ne 0) {
            Fail "npm dependency install failed in $Directory"
        }
    } finally {
        Pop-Location
    }
}

function Invoke-IntercomNpmInstall {
    $directory = Join-Path $ShareDir "intercom"
    $verifier = Join-Path (Join-Path $SourceDir "scripts") "verify-intercom-dependency-topology.mjs"
    $materializer = Join-Path (Join-Path $directory "scripts") "materialize-local-dependencies.mjs"

    foreach ($required in @("package.json", "package-lock.json", ".npmrc")) {
        if (-not (Test-Path -Path (Join-Path $directory $required) -PathType Leaf)) {
            Fail "missing Intercom root dependency file: $(Join-Path $directory $required)"
        }
    }
    if (-not (Test-Path -Path $verifier -PathType Leaf)) {
        Fail "missing Intercom dependency topology verifier: $verifier"
    }
    if (-not (Test-Path -Path $materializer -PathType Leaf)) {
        Fail "missing Intercom local dependency materializer: $materializer"
    }

    Write-Log "installing root-authoritative runtime dependencies in $directory"
    Push-Location $directory
    try {
        & npm "ci" "--omit=dev" "--install-links=true"
        if ($LASTEXITCODE -ne 0) {
            Fail "npm dependency install failed in $directory"
        }
    } finally {
        Pop-Location
    }
    & node $materializer $directory
    if ($LASTEXITCODE -ne 0) {
        Fail "Intercom local dependency materialization failed in $directory"
    }
    & node $verifier $directory
    if ($LASTEXITCODE -ne 0) {
        Fail "Intercom dependency topology verification failed in $directory"
    }
}

function Hydrate-RuntimeAssets {
    if ($SkipNode) {
        Write-Log "skipping runtime dependency install because -SkipNode was set"
        return
    }
    Ensure-Node
    Invoke-IntercomNpmInstall
    Invoke-NpmInstall -Directory (Join-Path $ShareDir "contracts")
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
    if (-not $UnsignedLayout) {
        Assert-SignedReleaseSelection
    }
    $archive = Get-ArtifactPath -Target $target
    if (-not (Test-Path $archive)) {
        Fail "artifact not found: $archive"
    }

    if ($UnsignedLayout) {
        Write-Warn "installing an explicit unsigned test layout; updater trust will not be provisioned"
        Verify-Archive -ArchivePath $archive
        $extractDir = New-TempDir
        Expand-MayhemArchive -ArchivePath $archive -Destination $extractDir
        $verifiedPackage = Verify-ExtractedChecksums -ExtractDir $extractDir
        Copy-ArtifactBins -PackageRoot $verifiedPackage.Root -VerifiedFiles $verifiedPackage.Files
        Copy-PackageAssets -PackageRoot $verifiedPackage.Root -VerifiedFiles $verifiedPackage.Files
    } else {
        Resolve-SignedReleaseMetadata -ArchivePath $archive
        Install-SignedRelease -ArchivePath $archive -Target $target
    }
}

function Install-SourceBinary {
    param(
        [string]$Source,
        [string]$Destination
    )

    Assert-RealFile -Path $Source -Label "locally built binary"
    $temporary = Join-Path $InstallDir (
        "." + [System.IO.Path]::GetFileName($Destination) +
        ".install." + [System.Guid]::NewGuid().ToString("N")
    )
    try {
        $input = [System.IO.File]::Open(
            $Source,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::Read
        )
        try {
            $output = [System.IO.File]::Open(
                $temporary,
                [System.IO.FileMode]::CreateNew,
                [System.IO.FileAccess]::Write,
                [System.IO.FileShare]::None
            )
            try {
                $input.CopyTo($output)
                $output.Flush($true)
            } finally {
                $output.Dispose()
            }
        } finally {
            $input.Dispose()
        }

        $zone = @(Get-Item -LiteralPath $temporary -Stream "Zone.Identifier" -ErrorAction SilentlyContinue)
        if ($zone.Count -ne 0) {
            Fail "locally built binary retained Windows Zone.Identifier metadata: $Source"
        }
        Move-Item -LiteralPath $temporary -Destination $Destination -Force
    } finally {
        Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
    }
}

function Install-FromSource {
    if (-not (Test-Path (Join-Path $SourceDir "Cargo.toml"))) {
        Fail "source dir does not contain Cargo.toml: $SourceDir"
    }
    if (-not (Test-Command "cargo")) {
        Fail "Rust/Cargo is required for -FromSource installs"
    }

    $targetTriple = Get-TargetTriple
    $selection = Resolve-LlamaCppSourceBuild -TargetTriple $targetTriple
    $features = @($selection.Features)
    $script:SourceLlamaCppBackend = $selection.Backend
    $targetDir = New-WindowsSourceBuildTargetDir
    Initialize-WindowsSourceBuildEnvironment -TargetTriple $targetTriple
    Write-Log "building release binaries from $SourceDir in private target directory $targetDir"
    Push-Location $SourceDir
    try {
        $featureArgs = @(
            Get-LlamaCppFeatureArgs `
                -Features $features `
                -TargetTriple $targetTriple
        )
        $cargoBuildArgs = @(
            Get-WindowsSourceBuildCargoArgs -LlamaCppFeatures $features
        )
        if ($cargoBuildArgs.Count -gt 0) {
            Write-Log "serializing Cargo jobs for the Windows Vulkan CMake/MSBuild external project"
        }
        & cargo build --release --workspace --bins `
            --target-dir $targetDir `
            @cargoBuildArgs `
            @featureArgs
        if ($LASTEXITCODE -ne 0) {
            Fail "cargo build failed"
        }
    } finally {
        Pop-Location
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    foreach ($bin in $Bins) {
        $src = Join-Path (Join-Path $targetDir "release") "$bin.exe"
        if (-not (Test-Path $src)) {
            Fail "missing built binary: $src"
        }
        Install-SourceBinary `
            -Source $src `
            -Destination (Join-Path $InstallDir "$bin.exe")
    }
    Copy-SourceAssets
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

    Ensure-Node
    New-Item -ItemType Directory -Path $NpmPrefix -Force | Out-Null
    Add-PathEntry -Entry $NpmPrefix
    $npmBin = Join-Path $NpmPrefix "bin"
    if (Test-Path $npmBin) {
        Add-PathEntry -Entry $npmBin
    }

    $packageJson = $null
    $versionCheck = @'
const fs = require("node:fs");
const metadata = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
process.exit(metadata.version === process.argv[2] ? 0 : 1);
'@
    foreach ($candidate in @(
        (Join-Path (Join-Path $NpmPrefix "node_modules") "pear\package.json"),
        (Join-Path (Join-Path (Join-Path $NpmPrefix "lib") "node_modules") "pear\package.json")
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            Assert-RealFile -Path $candidate -Label "Pear package metadata"
            & node "-e" $versionCheck $candidate $PearVersion
            if ($LASTEXITCODE -eq 0) {
                $packageJson = $candidate
                break
            }
        }
    }
    if ($null -eq $packageJson) {
        Write-Log "installing pinned Pear $PearVersion with npm prefix $NpmPrefix"
        & npm install -g "pear@$PearVersion" --prefix $NpmPrefix
        if ($LASTEXITCODE -ne 0) {
            Fail "npm failed to install pinned Pear $PearVersion"
        }
        foreach ($candidate in @(
            (Join-Path (Join-Path $NpmPrefix "node_modules") "pear\package.json"),
            (Join-Path (Join-Path (Join-Path $NpmPrefix "lib") "node_modules") "pear\package.json")
        )) {
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                Assert-RealFile -Path $candidate -Label "Pear package metadata"
                & node "-e" $versionCheck $candidate $PearVersion
                if ($LASTEXITCODE -eq 0) {
                    $packageJson = $candidate
                    break
                }
            }
        }
        if ($null -eq $packageJson) {
            Fail "npm did not install the pinned Pear $PearVersion package"
        }
    } else {
        Write-Log "found pinned Pear $PearVersion in $NpmPrefix"
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

function Confirm-SourceLlamaCppBackend {
    param(
        [string]$MayhemPath,
        [string]$Backend
    )

    $doctorHome = New-TempDir
    $doctorArgs = @(
        "doctor",
        "--provider-backend", "llama.cpp",
        "--home", $doctorHome,
        "--skip-disk-bench"
    )
    switch ($Backend) {
        "cuda" {
            $doctorArgs += @("--fixture", "linux-nvidia", "--gpu-layers", "1")
        }
        "vulkan" {
            # There is no Vulkan fixture, and NVIDIA devices may be classified as CUDA.
            # The successful feature build proves Vulkan; doctor proves its CPU fallback.
            $doctorArgs += @("--fixture", "cpu-only", "--gpu-layers", "0")
        }
        "cpu" {
            $doctorArgs += @("--fixture", "cpu-only", "--gpu-layers", "0")
        }
        default {
            Fail "unknown selected source llama.cpp backend: $Backend"
        }
    }

    $output = (& $MayhemPath @doctorArgs 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        if (-not [string]::IsNullOrWhiteSpace($output)) {
            Write-Host $output.TrimEnd()
        }
        Fail "installed source build failed llama.cpp $Backend backend verification"
    }
    if ($Backend -eq "vulkan") {
        Write-Log "verified installed llama.cpp Vulkan feature build and deterministic CPU fallback"
    } else {
        Write-Log "verified installed llama.cpp source backend: $Backend"
    }
}

function Smoke-Test {
    $mayhem = Join-Path $InstallDir "mayhem.exe"
    & $mayhem --version *> $null
    if ($LASTEXITCODE -ne 0) {
        Fail "installed mayhem binary did not report its version"
    }
    & $mayhem --help *> $null
    if ($LASTEXITCODE -ne 0) {
        Fail "installed mayhem binary did not run"
    }
    if ($FromSource) {
        Confirm-SourceLlamaCppBackend `
            -MayhemPath $mayhem `
            -Backend $script:SourceLlamaCppBackend
    }
    Write-Log "mayhem CLI smoke test passed"
}

function Main {
    if ($AllowUnverified) {
        Fail "-AllowUnverified and MAYHEM_ALLOW_UNVERIFIED have been removed; unsigned production installs are disabled"
    }
    if (-not $FromSource -and
        [string]::IsNullOrWhiteSpace($Artifact) -and
        [string]::IsNullOrWhiteSpace($ArtifactUrl) -and
        [string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
        if ((Test-Path (Join-Path $SourceDir "Cargo.toml")) -and (Test-Path (Join-Path (Join-Path $SourceDir "crates") "mayhem-cli"))) {
            $script:FromSource = $true
            $FromSource = $true
        }
    }
    if ($UnsignedLayout -and $FromSource) {
        Fail "-UnsignedLayout applies only to test release archives"
    }

    Add-PathEntry -Entry $InstallDir

    if ($FromSource) {
        Ensure-Pear
        Install-FromSource
        Hydrate-RuntimeAssets
    } else {
        Install-FromArtifact
        Ensure-Pear
    }

    Install-Opencode
    Update-UserPath
    Smoke-Test
    Write-Log "installed Mayhem binaries into $InstallDir"
    Write-Log "installed Mayhem runtime assets into $ShareDir"
}

try {
    Main
} finally {
    foreach ($dir in $script:TempDirs) {
        Remove-Item -Path $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
