param(
    [string]$InstallerPath = $(Join-Path (Join-Path $PSScriptRoot "..\..") "install.ps1")
)

$ErrorActionPreference = "Stop"

function Assert-Equal {
    param(
        $Actual,
        $Expected,
        [string]$Message
    )

    if ($Actual -cne $Expected) {
        throw "$Message (expected '$Expected', got '$Actual')"
    }
}

function Assert-True {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

$wantedFunctions = @(
    "Fail",
    "Test-Command",
    "Write-Log",
    "Get-WindowsSourceBuildCargoArgs",
    "Get-WindowsSourceBuildArchitecture",
    "Get-VsWherePath",
    "Find-VisualStudioSourceBuildTools",
    "Get-VsDevCmdBatchContent",
    "Set-ProcessEnvironmentFromSnapshot",
    "Test-VsArchitectureValue",
    "Assert-WindowsSourceBuildEnvironment",
    "Initialize-WindowsSourceBuildEnvironment",
    "Get-WindowsLocalAppDataPath",
    "New-WindowsSourceBuildTargetDir",
    "Install-FromSource"
)
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $InstallerPath,
    [ref]$tokens,
    [ref]$errors)
if ($errors.Count -gt 0) {
    throw "install.ps1 did not parse: $($errors[0].Message)"
}
$definitions = @{}
foreach ($definition in $ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst]
}, $true)) {
    if ($wantedFunctions -contains $definition.Name) {
        $definitions[$definition.Name] = $definition.Extent.Text
    }
}
foreach ($name in $wantedFunctions) {
    if (-not $definitions.ContainsKey($name)) {
        throw "install.ps1 is missing function $name"
    }
    Invoke-Expression $definitions[$name]
}

$sourceBuild = $definitions["Install-FromSource"]
Assert-True `
    $sourceBuild.Contains("New-WindowsSourceBuildTargetDir") `
    "source install does not allocate a short private build directory"
Assert-True `
    $sourceBuild.Contains("--target-dir `$targetDir") `
    "source install does not pass its private target directory to Cargo"
Assert-True `
    $sourceBuild.Contains("Get-WindowsSourceBuildCargoArgs") `
    "source install does not apply Windows native-build scheduling safeguards"
Assert-True `
    $sourceBuild.Contains('Join-Path $targetDir "release"') `
    "source install does not copy binaries from its private target directory"
$targetAllocationIndex = $sourceBuild.IndexOf(
    '$targetDir = New-WindowsSourceBuildTargetDir')
$environmentInitializationIndex = $sourceBuild.IndexOf(
    "Initialize-WindowsSourceBuildEnvironment")
Assert-True `
    ($targetAllocationIndex -ge 0 -and
        $targetAllocationIndex -lt $environmentInitializationIndex) `
    "source install must reserve its short build directory before importing the Visual Studio environment"
$targetDirectoryFactory = $definitions["New-WindowsSourceBuildTargetDir"]
Assert-True `
    $targetDirectoryFactory.Contains("Get-WindowsLocalAppDataPath") `
    "source build target discovery does not use the current user's known folder"
Assert-True `
    (-not $targetDirectoryFactory.Contains("C:\")) `
    "source build target discovery contains a hardcoded machine path"
$knownFolderLookup = $definitions["Get-WindowsLocalAppDataPath"]
Assert-True `
    $knownFolderLookup.Contains("GetUserProfileDirectoryW") `
    "Local AppData discovery does not use the current access token's profile"
Assert-True `
    $knownFolderLookup.Contains('Join-Path $profilePath "AppData\Local"') `
    "Local AppData discovery does not derive its path from the token profile"
Assert-True `
    (-not $knownFolderLookup.Contains("SHGetKnownFolderPath")) `
    "Local AppData discovery still trusts environment-sensitive shell folder lookup"
Assert-True `
    (-not $knownFolderLookup.Contains('$env:USERPROFILE')) `
    "Local AppData discovery trusts the mutable USERPROFILE environment variable"
Assert-True `
    (-not $knownFolderLookup.Contains("C:\")) `
    "Local AppData discovery contains a hardcoded machine path"

$x64 = Get-WindowsSourceBuildArchitecture `
    -TargetTriple "x86_64-pc-windows-msvc"
Assert-Equal $x64.VsTarget "amd64" "x86_64 target must select native amd64 tools"
Assert-Equal $x64.VsHost "amd64" "x86_64 host must select native amd64 tools"
Assert-Equal `
    $x64.RequiredComponent `
    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64" `
    "x86_64 discovery must require the x64 MSVC component"

$arm64 = Get-WindowsSourceBuildArchitecture `
    -TargetTriple "aarch64-pc-windows-msvc"
Assert-Equal $arm64.VsTarget "arm64" "ARM64 target must select native ARM64 tools"
Assert-Equal $arm64.VsHost "arm64" "ARM64 host must select native ARM64 tools"
Assert-Equal `
    $arm64.RequiredComponent `
    "Microsoft.VisualStudio.Component.VC.Tools.ARM64" `
    "ARM64 discovery must require the ARM64 MSVC component"

$x64Batch = Get-VsDevCmdBatchContent -Architecture $x64
Assert-True `
    $x64Batch.Contains("-arch=amd64 -host_arch=amd64") `
    "x86_64 developer environment arguments are incorrect"
$arm64Batch = Get-VsDevCmdBatchContent -Architecture $arm64
Assert-True `
    $arm64Batch.Contains("-arch=arm64 -host_arch=arm64") `
    "ARM64 developer environment arguments are incorrect"
Assert-True `
    (-not $x64Batch.Contains("Program Files")) `
    "developer environment bootstrap contains a hardcoded installation path"
Assert-True `
    $x64Batch.Contains('"%_MAYHEM_VSDEVCMD_PATH%"') `
    "developer environment bootstrap does not use the discovered script"

$cudaCargoArgs = @(
    Get-WindowsSourceBuildCargoArgs `
        -LlamaCppFeatures @("mayhem-cli/llama-cpp-cuda")
)
Assert-Equal `
    $cudaCargoArgs.Count `
    0 `
    "CUDA-only source builds must retain Cargo's normal parallelism"
$vulkanCargoArgs = @(
    Get-WindowsSourceBuildCargoArgs `
        -LlamaCppFeatures @(
            "mayhem-cli/llama-cpp-cuda",
            "mayhem-cli/llama-cpp-vulkan"
        )
)
Assert-Equal `
    ($vulkanCargoArgs -join " ") `
    "--jobs 1" `
    "Windows Vulkan source builds must serialize the nested MSBuild project"

$temp = Join-Path ([IO.Path]::GetTempPath()) (
    "mayhem-windows-build-env-test-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $temp | Out-Null
$oldProgramFilesX86 = [Environment]::GetEnvironmentVariable(
    "ProgramFiles(x86)",
    "Process")
$oldProgramFiles = [Environment]::GetEnvironmentVariable(
    "ProgramFiles",
    "Process")
$oldArgumentLog = $env:MAYHEM_VSWHERE_ARGUMENT_LOG
$oldFakeInstall = $env:MAYHEM_FAKE_VS_INSTALL
$oldSnapshotValue = $env:MAYHEM_WINDOWS_ENV_TEST
$oldHelperValue = $env:_MAYHEM_VSENV_OUTPUT
try {
    $locatorRoot = Join-Path $temp "locator"
    $locator = Join-Path $locatorRoot `
        "Microsoft Visual Studio\Installer\vswhere.exe"
    New-Item -ItemType Directory -Path (Split-Path -Parent $locator) -Force |
        Out-Null
    Set-Content -LiteralPath $locator -Value "placeholder" -NoNewline
    [Environment]::SetEnvironmentVariable(
        "ProgramFiles(x86)",
        $locatorRoot,
        "Process")
    [Environment]::SetEnvironmentVariable(
        "ProgramFiles",
        $null,
        "Process")
    Assert-Equal `
        (Get-VsWherePath) `
        $locator `
        "vswhere fallback discovery did not use the official dynamic root"

    $fakeInstall = Join-Path $temp "Visual Studio Build Tools"
    $fakeVsDevCmd = Join-Path $fakeInstall "Common7\Tools\VsDevCmd.bat"
    New-Item -ItemType Directory `
        -Path (Split-Path -Parent $fakeVsDevCmd) `
        -Force |
        Out-Null
    Set-Content -LiteralPath $fakeVsDevCmd -Value "@echo off" -NoNewline

    $fakeVsWhere = Join-Path $temp "fake-vswhere.exe"
    Set-Content -LiteralPath $fakeVsWhere -Value @'
#!/bin/sh
printf '%s\n' "$@" > "$MAYHEM_VSWHERE_ARGUMENT_LOG"
printf '%s\n' "$MAYHEM_FAKE_VS_INSTALL"
'@
    & /bin/chmod +x $fakeVsWhere
    if ($LASTEXITCODE -ne 0) {
        throw "could not make fake vswhere executable"
    }
    $argumentLog = Join-Path $temp "vswhere-arguments.txt"
    $env:MAYHEM_VSWHERE_ARGUMENT_LOG = $argumentLog
    $env:MAYHEM_FAKE_VS_INSTALL = $fakeInstall

    $x64Tools = Find-VisualStudioSourceBuildTools `
        -Architecture $x64 `
        -VsWherePath $fakeVsWhere
    Assert-Equal `
        $x64Tools.InstallationPath `
        $fakeInstall `
        "x86_64 tool discovery returned the wrong installation"
    Assert-Equal `
        $x64Tools.VsDevCmd `
        $fakeVsDevCmd `
        "x86_64 tool discovery returned the wrong VsDevCmd path"
    $x64Arguments = @(Get-Content -LiteralPath $argumentLog)
    Assert-True `
        ($x64Arguments -contains "Microsoft.Component.MSBuild") `
        "x86_64 discovery did not require MSBuild"
    Assert-True `
        ($x64Arguments -contains $x64.RequiredComponent) `
        "x86_64 discovery did not require its native MSVC component"

    $null = Find-VisualStudioSourceBuildTools `
        -Architecture $arm64 `
        -VsWherePath $fakeVsWhere
    $arm64Arguments = @(Get-Content -LiteralPath $argumentLog)
    Assert-True `
        ($arm64Arguments -contains "Microsoft.Component.MSBuild") `
        "ARM64 discovery did not require MSBuild"
    Assert-True `
        ($arm64Arguments -contains $arm64.RequiredComponent) `
        "ARM64 discovery did not require its native MSVC component"

    $snapshot = [pscustomobject]@{
        MAYHEM_WINDOWS_ENV_TEST = "left=right"
        _MAYHEM_VSENV_OUTPUT = "must-not-leak"
    }
    Set-ProcessEnvironmentFromSnapshot -Snapshot $snapshot
    Assert-Equal `
        $env:MAYHEM_WINDOWS_ENV_TEST `
        "left=right" `
        "environment snapshot import truncated a value containing '='"
    Assert-Equal `
        $env:_MAYHEM_VSENV_OUTPUT `
        $oldHelperValue `
        "environment snapshot import leaked a bootstrap helper variable"

    Assert-True `
        (Test-VsArchitectureValue -Actual "x64" -Expected "amd64") `
        "VsDevCmd x64 alias was not accepted for amd64"
    Assert-True `
        (Test-VsArchitectureValue -Actual "arm64" -Expected "arm64") `
        "VsDevCmd ARM64 architecture was not accepted"
    Assert-True `
        (-not (Test-VsArchitectureValue -Actual "x64" -Expected "arm64")) `
        "VsDevCmd architecture validation accepted the wrong target"

    $script:initializedArchitectures = @()
    function Find-VisualStudioSourceBuildTools {
        param([pscustomobject]$Architecture)
        $script:initializedArchitectures += $Architecture
        return [pscustomobject]@{
            InstallationPath = "discovered-install"
            VsDevCmd = "discovered-vsdevcmd"
        }
    }
    function Import-VisualStudioDeveloperEnvironment {
        param(
            [string]$VsDevCmd,
            [pscustomobject]$Architecture
        )
        Assert-Equal `
            $VsDevCmd `
            "discovered-vsdevcmd" `
            "initializer did not use the discovered VsDevCmd"
    }
    function Assert-WindowsSourceBuildEnvironment {
        param([pscustomobject]$Architecture)
    }
    function Write-Log {
        param([string]$Message)
    }

    Initialize-WindowsSourceBuildEnvironment `
        -TargetTriple "x86_64-pc-windows-msvc"
    Initialize-WindowsSourceBuildEnvironment `
        -TargetTriple "aarch64-pc-windows-msvc"
    Assert-Equal `
        $script:initializedArchitectures[0].VsTarget `
        "amd64" `
        "initializer did not wire the x86_64 architecture"
    Assert-Equal `
        $script:initializedArchitectures[1].VsTarget `
        "arm64" `
        "initializer did not wire the ARM64 architecture"
} finally {
    [Environment]::SetEnvironmentVariable(
        "ProgramFiles(x86)",
        $oldProgramFilesX86,
        "Process")
    [Environment]::SetEnvironmentVariable(
        "ProgramFiles",
        $oldProgramFiles,
        "Process")
    $env:MAYHEM_VSWHERE_ARGUMENT_LOG = $oldArgumentLog
    $env:MAYHEM_FAKE_VS_INSTALL = $oldFakeInstall
    $env:MAYHEM_WINDOWS_ENV_TEST = $oldSnapshotValue
    $env:_MAYHEM_VSENV_OUTPUT = $oldHelperValue
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "windows-source-build-environment.test: ok"
