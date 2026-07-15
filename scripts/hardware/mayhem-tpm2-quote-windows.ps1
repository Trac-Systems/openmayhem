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
$targetFramework = "net$dotnetMajor.0-windows"

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
$buildStampPath = Join-Path $helperRoot "build-source.sha256"

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
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text.Json;
using Microsoft.Win32;
using Tpm2Lib;

sealed class WindowsEkEvidence
{
    const string ProviderName = "Microsoft Platform Crypto Provider";
    const string EndorsementPath = @"SYSTEM\CurrentControlSet\Services\TPM\WMI\Endorsement";
    const uint RsaPublicMagic = 0x31415352;

    static readonly HashSet<uint> EccPublicMagics = new()
    {
        0x314b4345,
        0x334b4345,
        0x354b4345,
        0x31534345,
        0x33534345,
        0x35534345,
    };

    [DllImport("ncrypt.dll", CharSet = CharSet.Unicode)]
    static extern int NCryptOpenStorageProvider(
        out IntPtr provider,
        string providerName,
        uint flags);

    [DllImport("ncrypt.dll", CharSet = CharSet.Unicode)]
    static extern int NCryptGetProperty(
        IntPtr handle,
        string property,
        [Out] byte[]? output,
        int outputLength,
        out int resultLength,
        uint flags);

    [DllImport("ncrypt.dll")]
    static extern int NCryptFreeObject(IntPtr handle);

    public byte[] PublicBlob { get; private set; } = Array.Empty<byte>();
    public string DeviceKey { get; private set; } = string.Empty;
    public X509Certificate2? Certificate { get; private set; }
    public IReadOnlyList<byte[]> Chain { get; private set; } = Array.Empty<byte[]>();

    public static WindowsEkEvidence Read()
    {
        var publicBlob = ReadPcpPublicBlob();
        var identityBytes = PublicIdentityBytes(publicBlob);
        var certificates = ReadRegisteredCertificates();
        var certificate = certificates.FirstOrDefault(cert => CertificateMatches(cert, publicBlob));
        if (certificate is not null && !certificate.GetPublicKey().SequenceEqual(identityBytes))
        {
            throw new InvalidOperationException("Windows TPM EK certificate public key does not match PCP_EKPUB");
        }

        var deviceKey = Convert.ToHexString(SHA256.HashData(identityBytes)).ToLowerInvariant();
        var chain = new List<byte[]>();
        if (certificate is not null)
        {
            chain.Add(certificate.RawData);
            using var builder = new X509Chain();
            builder.ChainPolicy.RevocationMode = X509RevocationMode.NoCheck;
            builder.Build(certificate);
            foreach (var element in builder.ChainElements.Cast<X509ChainElement>())
            {
                if (!chain.Any(existing => existing.SequenceEqual(element.Certificate.RawData)))
                {
                    chain.Add(element.Certificate.RawData);
                }
            }
        }

        return new WindowsEkEvidence
        {
            PublicBlob = publicBlob,
            DeviceKey = deviceKey,
            Certificate = certificate,
            Chain = chain,
        };
    }

    static byte[] ReadPcpPublicBlob()
    {
        var status = NCryptOpenStorageProvider(out var provider, ProviderName, 0);
        if (status != 0)
        {
            throw new InvalidOperationException($"NCryptOpenStorageProvider failed (0x{unchecked((uint)status):x8})");
        }
        try
        {
            foreach (var property in new[] { "PCP_EKPUB", "PCP_RSA_EKPUB", "PCP_ECC_EKPUB" })
            {
                status = NCryptGetProperty(provider, property, null, 0, out var length, 0);
                if (status != 0 || length <= 0)
                {
                    continue;
                }
                var bytes = new byte[length];
                status = NCryptGetProperty(provider, property, bytes, bytes.Length, out length, 0);
                if (status == 0 && length > 0)
                {
                    if (length != bytes.Length)
                    {
                        Array.Resize(ref bytes, length);
                    }
                    return bytes;
                }
            }
            throw new InvalidOperationException("Windows Platform Crypto Provider did not expose a TPM EK public key");
        }
        finally
        {
            NCryptFreeObject(provider);
        }
    }

    static byte[] PublicIdentityBytes(byte[] blob)
    {
        if (blob.Length < 8)
        {
            throw new InvalidOperationException("Windows TPM EK public blob is truncated");
        }
        var magic = BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(0, 4));
        if (magic == RsaPublicMagic)
        {
            var parameters = RsaParameters(blob);
            using var rsa = RSA.Create();
            rsa.ImportParameters(parameters);
            return rsa.ExportRSAPublicKey();
        }
        if (EccPublicMagics.Contains(magic))
        {
            var keyLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(4, 4)));
            if (keyLength <= 0 || blob.Length != 8 + (keyLength * 2))
            {
                throw new InvalidOperationException("Windows TPM ECC EK public blob has an invalid length");
            }
            return new byte[] { 0x04 }
                .Concat(blob.AsSpan(8, keyLength).ToArray())
                .Concat(blob.AsSpan(8 + keyLength, keyLength).ToArray())
                .ToArray();
        }
        throw new InvalidOperationException($"unsupported Windows TPM EK public blob magic 0x{magic:x8}");
    }

    static RSAParameters RsaParameters(byte[] blob)
    {
        if (blob.Length < 24)
        {
            throw new InvalidOperationException("Windows TPM RSA EK public blob is truncated");
        }
        var exponentLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(8, 4)));
        var modulusLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(12, 4)));
        if (exponentLength <= 0 || modulusLength <= 0 || blob.Length != 24 + exponentLength + modulusLength)
        {
            throw new InvalidOperationException("Windows TPM RSA EK public blob has an invalid length");
        }
        return new RSAParameters
        {
            Exponent = blob.AsSpan(24, exponentLength).ToArray(),
            Modulus = blob.AsSpan(24 + exponentLength, modulusLength).ToArray(),
        };
    }

    static bool CertificateMatches(X509Certificate2 certificate, byte[] publicBlob)
    {
        var magic = BinaryPrimitives.ReadUInt32LittleEndian(publicBlob.AsSpan(0, 4));
        if (magic == RsaPublicMagic)
        {
            using var rsa = certificate.GetRSAPublicKey();
            if (rsa is null)
            {
                return false;
            }
            var expected = RsaParameters(publicBlob);
            var actual = rsa.ExportParameters(false);
            return actual.Exponent.AsSpan().SequenceEqual(expected.Exponent) &&
                   actual.Modulus.AsSpan().SequenceEqual(expected.Modulus);
        }
        if (EccPublicMagics.Contains(magic))
        {
            using var ecdsa = certificate.GetECDsaPublicKey();
            if (ecdsa is null)
            {
                return false;
            }
            var keyLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(publicBlob.AsSpan(4, 4)));
            var actual = ecdsa.ExportParameters(false);
            return actual.Q.X.AsSpan().SequenceEqual(publicBlob.AsSpan(8, keyLength)) &&
                   actual.Q.Y.AsSpan().SequenceEqual(publicBlob.AsSpan(8 + keyLength, keyLength));
        }
        return false;
    }

    static List<X509Certificate2> ReadRegisteredCertificates()
    {
        var certificates = new List<X509Certificate2>();
        foreach (var store in new[] { "EKCertStore", "EKCertStoreECC" })
        {
            using var root = Registry.LocalMachine.OpenSubKey(
                EndorsementPath + "\\" + store + "\\Certificates",
                false);
            if (root is null)
            {
                continue;
            }
            foreach (var thumbprint in root.GetSubKeyNames())
            {
                using var key = root.OpenSubKey(thumbprint, false);
                if (key?.GetValue("Blob") is not byte[] blob)
                {
                    continue;
                }
                var collection = new X509Certificate2Collection();
#pragma warning disable SYSLIB0057
                collection.Import(blob);
#pragma warning restore SYSLIB0057
                foreach (var certificate in collection)
                {
                    if (!certificates.Any(existing => existing.Thumbprint == certificate.Thumbprint))
                    {
                        certificates.Add(certificate);
                    }
                }
            }
        }
        return certificates;
    }
}

static class Program
{
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

public static void Main()
{
var binding = NeedEnv("MAYHEM_HW_QUOTE_BINDING").ToLowerInvariant();
var bindingBytes = HexToBytes(binding);
var ek = WindowsEkEvidence.Read();
var deviceKey = ek.DeviceKey;
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
        ["ek_public_bcrypt_b64"] = B64(ek.PublicBlob),
        ["device_key"] = deviceKey,
        ["quote_verified_locally"] = true
    };

    if (ek.Certificate is not null)
    {
        evidence["ek_cert_der_b64"] = B64(ek.Certificate.RawData);
        evidence["ek_cert_thumbprint"] = ek.Certificate.Thumbprint;
        evidence["ek_cert_subject"] = ek.Certificate.Subject;
        evidence["ek_cert_issuer"] = ek.Certificate.Issuer;
        if (ek.Chain.Count > 0)
        {
            evidence["ek_chain_der_b64"] = ek.Chain.Select(B64).ToArray();
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
            ["ek_cert_present"] = ek.Certificate is not null
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
}
}
'@

if (-not (Test-Path $csprojPath) -or ((Get-Content -Raw $csprojPath) -ne $csproj)) {
  Set-Content -Path $csprojPath -Value $csproj -Encoding UTF8
}
if (-not (Test-Path $programPath) -or ((Get-Content -Raw $programPath) -ne $program)) {
  Set-Content -Path $programPath -Value $program -Encoding UTF8
}

$sourceBytes = [Text.Encoding]::UTF8.GetBytes($csproj + "`n--PROGRAM--`n" + $program)
$sourceDigest = [Security.Cryptography.SHA256]::Create().ComputeHash($sourceBytes)
$sourceHash = -join ($sourceDigest | ForEach-Object { $_.ToString("x2") })
$outputRoot = Join-Path $helperRoot "bin\Release\$targetFramework"
$helperExe = Join-Path $outputRoot "MayhemTpm2QuoteWindows.exe"
$stampMatches = (Test-Path $buildStampPath) -and
  ((Get-Content -Raw $buildStampPath).Trim() -eq $sourceHash)

if (-not $stampMatches -or -not (Test-Path $helperExe)) {
  $rootDigest = [Security.Cryptography.SHA256]::Create().ComputeHash(
    [Text.Encoding]::UTF8.GetBytes($helperRoot.ToLowerInvariant())
  )
  $rootHash = -join ($rootDigest | ForEach-Object { $_.ToString("x2") })
  $buildMutex = New-Object Threading.Mutex($false, "Local\MayhemTpm2QuoteWindows-$($rootHash.Substring(0,16))")
  $mutexHeld = $false
  try {
    try {
      $mutexHeld = $buildMutex.WaitOne([TimeSpan]::FromMinutes(10))
    } catch [Threading.AbandonedMutexException] {
      $mutexHeld = $true
    }
    if (-not $mutexHeld) {
      Fail "timed out waiting for another TPM helper build"
    }
    $stampMatches = (Test-Path $buildStampPath) -and
      ((Get-Content -Raw $buildStampPath).Trim() -eq $sourceHash)
    if (-not $stampMatches -or -not (Test-Path $helperExe)) {
      & dotnet restore $csprojPath --nologo | Out-Null
      if ($LASTEXITCODE -ne 0) {
        Fail "dotnet restore failed for Microsoft.TSS"
      }
      & dotnet build $csprojPath -c Release --no-restore --nologo | Out-Null
      if ($LASTEXITCODE -ne 0 -or -not (Test-Path $helperExe)) {
        Fail "building the Windows TPM quote helper failed"
      }
      Set-Content -Path $buildStampPath -Value $sourceHash -Encoding ASCII
    }
  } finally {
    if ($mutexHeld) {
      $null = $buildMutex.ReleaseMutex()
    }
    $buildMutex.Dispose()
  }
}

& $helperExe
if ($LASTEXITCODE -ne 0) {
  Fail "Windows TPM quote generation failed"
}
