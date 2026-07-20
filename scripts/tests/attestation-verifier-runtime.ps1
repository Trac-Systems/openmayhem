param(
    [Parameter(Mandatory = $true)]
    [string]$Binary
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    throw "attestation-verifier-runtime.test: $Message"
}

$currentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
$currentPrincipal = New-Object Security.Principal.WindowsPrincipal($currentIdentity)
if ($currentPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Fail 'runtime verification must execute with a standard-user token'
}

function Invoke-Verifier(
    [string]$Executable,
    [string[]]$Arguments,
    [string]$InputPath = ''
) {
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $Executable
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables.Clear()
    $start.EnvironmentVariables['PATH'] = "$env:SystemRoot\System32;$env:SystemRoot"
    $start.EnvironmentVariables['SystemRoot'] = $env:SystemRoot
    $start.EnvironmentVariables['WINDIR'] = $env:WINDIR
    $start.EnvironmentVariables['TEMP'] = $env:TEMP
    $start.EnvironmentVariables['TMP'] = $env:TMP
    $start.Arguments = (($Arguments | ForEach-Object {
        '"' + $_.Replace('"', '\"') + '"'
    }) -join ' ')

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $start
    if (-not $process.Start()) {
        Fail 'could not start verifier'
    }
    if ($InputPath) {
        $input = [System.IO.File]::OpenRead($InputPath)
        try {
            try {
                $input.CopyTo($process.StandardInput.BaseStream)
            } catch [System.IO.IOException] {
                # The verifier deliberately closes stdin after its bounded read.
            }
        } finally {
            $input.Dispose()
        }
    }
    $process.StandardInput.Close()
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) {
        Fail "verifier process failed with exit $($process.ExitCode): $stderr"
    }
    return ($stdout | ConvertFrom-Json)
}

$resolved = (Resolve-Path -LiteralPath $Binary).Path
if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
    Fail "verifier binary is missing: $resolved"
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
    'mayhem-av2-windows-' + [Guid]::NewGuid().ToString('N')
)
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $installed = Join-Path $tempRoot 'mayhem-attestation-verifier.exe'
    Copy-Item -LiteralPath $resolved -Destination $installed
    (Get-Item -LiteralPath $installed).IsReadOnly = $true

    $image = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($installed))
    if ($image -match 'requireAdministrator|highestAvailable') {
        Fail 'Windows executable requests an elevated token'
    }

    $identity = Invoke-Verifier $installed @('--identity')
    $identityJson = $identity | ConvertTo-Json -Compress -Depth 8
    if ($identityJson.Length -gt 4096) {
        Fail 'identity output exceeds its 4 KiB bound'
    }
    if (
        $identity.verifier_id -ne 'mayhem-attestation-verifier' -or
        $identity.version -ne 1 -or
        $identity.max_input_bytes -ne (8 * 1024 * 1024) -or
        $identity.public_trust_source -ne 'authenticated_admin_policy_input'
    ) {
        Fail 'identity output has invalid verifier metadata'
    }
    foreach ($profile in @(
        'amd_sev_snp_vcek_v1',
        'intel_tdx_dcap_v1',
        'nvidia_nras_composite_v1',
        'nvidia_nvtrust_offline_composite_v1'
    )) {
        $schemas = $identity.profiles.$profile
        if ($null -eq $schemas -or @($schemas).Count -ne 1 -or $schemas -ne 1) {
            Fail "identity output omits $profile schema 1"
        }
    }
    if ($identityJson -match 'executable_sha256|"platform"|endpoint|jwks|trust_root') {
        Fail 'identity output contains platform-specific or caller-supplied trust authority'
    }

    $empty = Invoke-Verifier $installed @()
    if ($empty.ok -ne $false -or $empty.reason -notmatch 'strict verifier input JSON is invalid') {
        Fail 'empty stdin did not fail closed'
    }

    $manual = Invoke-Verifier $installed @('--trust-root', 'provider-root.pem')
    if ($manual.ok -ne $false -or $manual.reason -notmatch 'only accepts --identity') {
        Fail 'manual trust arguments were not rejected'
    }

    $oversizePath = Join-Path $tempRoot 'oversize.bin'
    $oversize = [System.IO.File]::Create($oversizePath)
    try {
        $oversize.SetLength(9 * 1024 * 1024)
    } finally {
        $oversize.Dispose()
    }
    $oversizeVerdict = Invoke-Verifier $installed @() $oversizePath
    if ($oversizeVerdict.ok -ne $false -or $oversizeVerdict.reason -notmatch '8 MiB limit') {
        Fail 'oversized stdin was not rejected'
    }

    $root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
    $source = Get-ChildItem (
        Join-Path $root 'crates\mayhem-attestation-verifier\src'
    ) -Filter '*.rs'
    $externalMatches = @($source | Select-String -Pattern (
        'std::process::Command|Command::new|std::fs|fs::|File::|OpenOptions|' +
        'PathBuf|powershell|python|dotnet|sudo|runas'
    ) -CaseSensitive)
    if ($externalMatches.Count -ne 0) {
        $match = $externalMatches[0]
        Fail (
            'verifier runtime contains an external-command, filesystem-trust, or ' +
            'elevation dependency at ' +
            "$($match.Path):$($match.LineNumber): $($match.Line.Trim())"
        )
    }
    $bins = Get-Content -Raw (Join-Path $root 'scripts\package-release.sh')
    if ($bins -notmatch '(?m)^\s+mayhem-attestation-verifier\s*$') {
        Fail 'release package inventory omits mayhem-attestation-verifier'
    }
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Get-ChildItem -LiteralPath $tempRoot -Force |
            ForEach-Object { $_.IsReadOnly = $false }
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}

Write-Output 'attestation-verifier-runtime.test: ok'
