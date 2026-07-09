param()

$ErrorActionPreference = "Stop"

function Fail($Message) {
  [Console]::Error.WriteLine("mayhem-tpm2-quote-windows: $Message")
  exit 1
}

$binding = $env:MAYHEM_HW_QUOTE_BINDING
if ([string]::IsNullOrWhiteSpace($binding)) {
  $binding = $env:MAYHEM_HW_QUOTE_NONCE
}
if ([string]::IsNullOrWhiteSpace($binding)) {
  Fail "MAYHEM_HW_QUOTE_BINDING is required"
}
if ($binding -notmatch '^[0-9a-fA-F]{64}$') {
  Fail "MAYHEM_HW_QUOTE_BINDING must be a 32-byte hex digest"
}
$binding = $binding.ToLowerInvariant()
$env:MAYHEM_HW_QUOTE_BINDING = $binding

if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
  Fail ".NET SDK is required for the Windows TPM helper"
}
if (-not (Get-Command Get-TpmEndorsementKeyInfo -ErrorAction SilentlyContinue)) {
  Fail "Get-TpmEndorsementKeyInfo is required to read the TPM EK identity"
}

$ekInfo = Get-TpmEndorsementKeyInfo -HashAlgorithm sha256
if (-not $ekInfo.IsPresent -or [string]::IsNullOrWhiteSpace($ekInfo.PublicKeyHash)) {
  Fail "TPM endorsement key public hash is not available"
}
$deviceKey = ([string]$ekInfo.PublicKeyHash).ToLowerInvariant()
if ($deviceKey -notmatch '^[0-9a-f]{64}$') {
  Fail "TPM endorsement key public hash is not a 32-byte hex digest"
}
$env:MAYHEM_TPM2_EK_PUBLIC_HASH = $deviceKey

$certs = @()
if ($ekInfo.ManufacturerCertificates) {
  $certs += @($ekInfo.ManufacturerCertificates)
}
if ($ekInfo.AdditionalCertificates) {
  $certs += @($ekInfo.AdditionalCertificates)
}
$cert = $certs | Select-Object -First 1
if ($cert) {
  $env:MAYHEM_TPM2_EK_CERT_DER_B64 = [Convert]::ToBase64String($cert.RawData)
  $env:MAYHEM_TPM2_EK_CERT_THUMBPRINT = $cert.Thumbprint
  $env:MAYHEM_TPM2_EK_CERT_SUBJECT = $cert.Subject
  $env:MAYHEM_TPM2_EK_CERT_ISSUER = $cert.Issuer
  try {
    $chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
    $chain.ChainPolicy.RevocationMode = [System.Security.Cryptography.X509Certificates.X509RevocationMode]::NoCheck
    [void]$chain.Build($cert)
    $chainB64 = @($chain.ChainElements | ForEach-Object {
      [Convert]::ToBase64String($_.Certificate.RawData)
    })
    if ($chainB64.Count -gt 0) {
      $env:MAYHEM_TPM2_EK_CHAIN_DER_B64_JSON = ($chainB64 | ConvertTo-Json -Compress)
    }
  } catch {
    $env:MAYHEM_TPM2_EK_CHAIN_DER_B64_JSON = ""
  }
}

try {
  $nvidiaSmi = Get-Command nvidia-smi.exe -ErrorAction SilentlyContinue
  if ($nvidiaSmi) {
    $rows = & $nvidiaSmi.Source --query-gpu=name,uuid,driver_version,vbios_version --format=csv,noheader,nounits 2>$null
    $gpus = @()
    foreach ($row in $rows) {
      $parts = $row -split '\s*,\s*'
      if ($parts.Length -ge 4) {
        $gpus += [ordered]@{
          name = $parts[0]
          uuid = $parts[1]
          driver = $parts[2]
          vbios = $parts[3]
        }
      }
    }
    if ($gpus.Count -gt 0) {
      $env:MAYHEM_TPM2_GPU_JSON = ($gpus | ConvertTo-Json -Compress)
    }
  }
} catch {
  $env:MAYHEM_TPM2_GPU_JSON = ""
}

$dotnetVersion = (& dotnet --version).Trim()
$dotnetMajor = ($dotnetVersion -split '\.')[0]
if ($dotnetMajor -notmatch '^[0-9]+$' -or [int]$dotnetMajor -lt 6) {
  Fail ".NET SDK 6 or newer is required; found $dotnetVersion"
}
$targetFramework = "net$dotnetMajor.0"

$helperRoot = $env:MAYHEM_TPM2_HELPER_DIR
if ([string]::IsNullOrWhiteSpace($helperRoot)) {
  $localAppData = $env:LOCALAPPDATA
  if ([string]::IsNullOrWhiteSpace($localAppData)) {
    $localAppData = [IO.Path]::GetTempPath()
  }
  $helperRoot = Join-Path $localAppData "Mayhem\tpm2-quote-windows"
}
New-Item -ItemType Directory -Force -Path $helperRoot | Out-Null

$csprojPath = Join-Path $helperRoot "MayhemTpm2QuoteWindows.csproj"
$programPath = Join-Path $helperRoot "Program.cs"

$csproj = @"
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>$targetFramework</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Microsoft.TSS" Version="2.1.1" />
  </ItemGroup>
</Project>
"@

$program = @'
using System;
using System.Collections.Generic;
using System.Linq;
using System.Security.Cryptography;
using System.Text.Json;
using Tpm2Lib;

static string NeedEnv(string name)
{
    var value = Environment.GetEnvironmentVariable(name);
    if (string.IsNullOrWhiteSpace(value))
    {
        throw new InvalidOperationException($"{name} is required");
    }
    return value.Trim();
}

static byte[] HexToBytes(string hex)
{
    if (hex.Length % 2 != 0)
    {
        throw new ArgumentException("hex length must be even");
    }
    var bytes = new byte[hex.Length / 2];
    for (var i = 0; i < bytes.Length; i++)
    {
        bytes[i] = Convert.ToByte(hex.Substring(i * 2, 2), 16);
    }
    return bytes;
}

static string B64(byte[] bytes) => Convert.ToBase64String(bytes);

static uint[] ParsePcrs(string selection)
{
    var parts = selection.Split(':', 2);
    var list = parts.Length == 2 ? parts[1] : selection;
    return list.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
        .Select(value => uint.Parse(value))
        .ToArray();
}

static TpmHandle CreateStoragePrimary(Tpm2 tpm, out TpmPublic publicArea)
{
    var sensitive = new SensitiveCreate(null, null);
    var template = new TpmPublic(
        TpmAlgId.Sha256,
        ObjectAttr.Restricted | ObjectAttr.Decrypt |
        ObjectAttr.FixedParent | ObjectAttr.FixedTPM |
        ObjectAttr.UserWithAuth | ObjectAttr.SensitiveDataOrigin,
        null,
        new RsaParms(new SymDefObject(TpmAlgId.Aes, 128, TpmAlgId.Cfb), null, 2048, 0),
        new Tpm2bPublicKeyRsa());
    return tpm.CreatePrimary(
        TpmRh.Owner,
        sensitive,
        template,
        null,
        Array.Empty<PcrSelection>(),
        out publicArea,
        out _,
        out _,
        out _);
}

static (TpmHandle Handle, TpmPublic Public) CreateQuoteKey(Tpm2 tpm, TpmHandle parent)
{
    var template = new TpmPublic(
        TpmAlgId.Sha256,
        ObjectAttr.Sign | ObjectAttr.Restricted |
        ObjectAttr.FixedParent | ObjectAttr.FixedTPM |
        ObjectAttr.UserWithAuth | ObjectAttr.SensitiveDataOrigin,
        null,
        new RsaParms(new SymDefObject(), new SchemeRsassa(TpmAlgId.Sha256), 2048, 0),
        new Tpm2bPublicKeyRsa());
    var sensitive = new SensitiveCreate(null, null);
    var privatePart = tpm.Create(
        parent,
        sensitive,
        template,
        null,
        Array.Empty<PcrSelection>(),
        out var publicPart,
        out _,
        out _,
        out _);
    var handle = tpm.Load(parent, privatePart, publicPart);
    return (handle, publicPart);
}

var binding = NeedEnv("MAYHEM_HW_QUOTE_BINDING").ToLowerInvariant();
var bindingBytes = HexToBytes(binding);
var deviceKey = NeedEnv("MAYHEM_TPM2_EK_PUBLIC_HASH").ToLowerInvariant();
var pcrSelectionText = Environment.GetEnvironmentVariable("MAYHEM_TPM2_PCR_SELECTION");
if (string.IsNullOrWhiteSpace(pcrSelectionText))
{
    pcrSelectionText = "sha256:0,2,4,7";
}
var pcrs = ParsePcrs(pcrSelectionText);

using var device = new TbsDevice();
device.Connect();
using var tpm = new Tpm2(device);
TpmHandle? primary = null;
TpmHandle? quoteKey = null;

try
{
    primary = CreateStoragePrimary(tpm, out var primaryPublic);
    var key = CreateQuoteKey(tpm, primary);
    quoteKey = key.Handle;
    var selections = new[] { new PcrSelection(TpmAlgId.Sha256, pcrs) };
    var quotedInfo = tpm.Quote(quoteKey, bindingBytes, new SchemeRsassa(TpmAlgId.Sha256), selections, out var quoteSig);
    tpm.PcrRead(selections, out var selectedPcrs, out var pcrValues);
    var quoteOk = key.Public.VerifyQuote(TpmAlgId.Sha256, selectedPcrs, pcrValues, bindingBytes, quotedInfo, quoteSig);
    if (!quoteOk)
    {
        throw new InvalidOperationException("TPM quote failed local verification");
    }

    var evidence = new Dictionary<string, object?>
    {
        ["schema_version"] = 1,
        ["kind"] = "tpm2_quote_ek",
        ["binding"] = binding,
        ["pcr_selection"] = pcrSelectionText,
        ["hash_algorithm"] = "sha256",
        ["tool"] = "tss.net/windows-tbs",
        ["ak_public_tpm_b64"] = B64(key.Public.GetTpmRepresentation()),
        ["ak_name_b64"] = B64(quoteKey.Name),
        ["quote_attest_b64"] = B64(quotedInfo.GetTpmRepresentation()),
        ["quote_signature_b64"] = B64(Marshaller.GetTpmRepresentation(quoteSig)),
        ["pcr_selection_tpm_b64"] = B64(Marshaller.GetTpmRepresentation(selectedPcrs)),
        ["pcr_values_tpm_b64"] = B64(Marshaller.GetTpmRepresentation(pcrValues)),
        ["device_key"] = deviceKey,
        ["quote_verified_locally"] = true
    };

    var certB64 = Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_CERT_DER_B64");
    if (!string.IsNullOrWhiteSpace(certB64))
    {
        evidence["ek_cert_der_b64"] = certB64;
        evidence["ek_cert_thumbprint"] = Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_CERT_THUMBPRINT");
        evidence["ek_cert_subject"] = Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_CERT_SUBJECT");
        evidence["ek_cert_issuer"] = Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_CERT_ISSUER");
        var chainJson = Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_CHAIN_DER_B64_JSON");
        if (!string.IsNullOrWhiteSpace(chainJson))
        {
            evidence["ek_chain_der_b64"] = JsonSerializer.Deserialize<JsonElement>(chainJson);
        }
    }

    object? gpu = null;
    var gpuJson = Environment.GetEnvironmentVariable("MAYHEM_TPM2_GPU_JSON");
    if (!string.IsNullOrWhiteSpace(gpuJson))
    {
        gpu = JsonSerializer.Deserialize<JsonElement>(gpuJson);
        evidence["gpu"] = gpu;
    }

    var output = new Dictionary<string, object?>
    {
        ["kind"] = "tpm2_quote_ek",
        ["binding"] = binding,
        ["evidence"] = JsonSerializer.Serialize(evidence),
        ["device_key"] = deviceKey,
        ["tpm"] = new Dictionary<string, object?>
        {
            ["ek_sha256"] = deviceKey,
            ["pcr_selection"] = pcrSelectionText,
            ["ek_cert_present"] = !string.IsNullOrWhiteSpace(certB64)
        }
    };
    if (gpu is not null)
    {
        output["gpu"] = gpu;
    }

    Console.WriteLine(JsonSerializer.Serialize(output));
}
finally
{
    if (quoteKey is not null)
    {
        try { tpm.FlushContext(quoteKey); } catch { }
    }
    if (primary is not null)
    {
        try { tpm.FlushContext(primary); } catch { }
    }
}
'@

if (-not (Test-Path $csprojPath) -or ((Get-Content -Raw $csprojPath) -ne $csproj)) {
  Set-Content -Path $csprojPath -Value $csproj -Encoding UTF8
}
if (-not (Test-Path $programPath) -or ((Get-Content -Raw $programPath) -ne $program)) {
  Set-Content -Path $programPath -Value $program -Encoding UTF8
}

& dotnet restore $csprojPath --nologo | Out-Null
if ($LASTEXITCODE -ne 0) {
  Fail "dotnet restore failed for Microsoft.TSS"
}

& dotnet run --project $csprojPath -c Release --no-restore --nologo
if ($LASTEXITCODE -ne 0) {
  Fail "Windows TPM quote generation failed"
}
