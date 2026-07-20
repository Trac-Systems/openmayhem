param(
  [Parameter(Mandatory = $true, Position = 0)]
  [ValidateSet("quote", "activate_credential")]
  [string]$Operation
)

$ErrorActionPreference = "Stop"

function Fail($Message) {
  [Console]::Error.WriteLine("mayhem-tpm2-quote-windows: $Message")
  exit 1
}

$stateRoot = $env:MAYHEM_TPM2_STATE_DIR
if ([string]::IsNullOrWhiteSpace($stateRoot)) {
  Fail "MAYHEM_TPM2_STATE_DIR is required"
}
if (-not [IO.Path]::IsPathRooted($stateRoot)) {
  Fail "MAYHEM_TPM2_STATE_DIR must be an absolute path"
}
try {
  $env:MAYHEM_TPM2_STATE_DIR = [IO.Path]::GetFullPath($stateRoot)
} catch {
  Fail "MAYHEM_TPM2_STATE_DIR is invalid"
}

$ekInfoCommand = Get-Command Get-TpmEndorsementKeyInfo -ErrorAction SilentlyContinue
if (-not $ekInfoCommand) {
  Fail "Get-TpmEndorsementKeyInfo is required"
}
try {
  $oldProgressPreference = $ProgressPreference
  $ProgressPreference = "SilentlyContinue"
  $ekInfo = Get-TpmEndorsementKeyInfo -ErrorAction Stop
  $ekCertificates = @($ekInfo.ManufacturerCertificates) +
    @($ekInfo.AdditionalCertificates)
  if ($ekCertificates.Count -eq 0 -or $ekCertificates.Count -gt 8) {
    Fail "Windows must expose between one and eight EK certificates"
  }
  $encodedEkCertificates = @(
    $ekCertificates | ForEach-Object {
      if ($_.RawData.Length -le 0 -or $_.RawData.Length -gt 131072) {
        Fail "Windows exposed an EK certificate outside the size bound"
      }
      [Convert]::ToBase64String($_.RawData)
    }
  )
  $env:MAYHEM_TPM2_EK_CERTIFICATES_JSON = ConvertTo-Json `
    -InputObject $encodedEkCertificates `
    -Compress
} catch {
  Fail "reading Windows TPM EK certificates failed: $($_.Exception.Message)"
} finally {
  $ProgressPreference = $oldProgressPreference
}

$dotnetCommand = Get-Command dotnet -ErrorAction SilentlyContinue
$dotnetPath = $null
if ($dotnetCommand) {
  $dotnetPath = $dotnetCommand.Path
}
if ([string]::IsNullOrWhiteSpace($dotnetPath)) {
  $userDotnet = Join-Path $HOME ".dotnet\dotnet.exe"
  if (Test-Path $userDotnet -PathType Leaf) {
    $dotnetPath = $userDotnet
  }
}
if ([string]::IsNullOrWhiteSpace($dotnetPath)) {
  Fail ".NET SDK is required for the Windows TPM helper"
}

$dotnetVersion = (& $dotnetPath --version).Trim()
$dotnetMajor = ($dotnetVersion -split '\.')[0]
if ($dotnetMajor -notmatch '^[0-9]+$' -or [int]$dotnetMajor -lt 6) {
  Fail ".NET SDK 6 or newer is required; found $dotnetVersion"
}
$targetFramework = "net$dotnetMajor.0-windows"

$helperRoot = $env:MAYHEM_TPM2_HELPER_DIR
if ([string]::IsNullOrWhiteSpace($helperRoot)) {
  if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    Fail "LOCALAPPDATA is required when MAYHEM_TPM2_HELPER_DIR is unset"
  }
  $helperRoot = Join-Path $env:LOCALAPPDATA "Mayhem\tpm2-provider-helper"
}
New-Item -ItemType Directory -Force -Path $helperRoot | Out-Null

$csprojPath = Join-Path $helperRoot "MayhemTpm2ProviderHelper.csproj"
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
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text;
using System.Text.Json;
using System.Threading;
using Tpm2Lib;

sealed class WindowsEkEvidence
{
    const string ProviderName = "Microsoft Platform Crypto Provider";
    const uint RsaPublicMagic = 0x31415352;

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

    public byte[] CanonicalSpki { get; private init; } = Array.Empty<byte>();
    public string DeviceKey { get; private init; } = string.Empty;
    public X509Certificate2 Certificate { get; private init; } = null!;
    public IReadOnlyList<byte[]> Chain { get; private init; } = Array.Empty<byte[]>();

    public static WindowsEkEvidence Read()
    {
        var pcpBlob = ReadPcpRsaPublicBlob();
        var pcpSpki = RsaSpki(RsaParameters(pcpBlob));
        var candidates = ReadEkCertificates().ToArray();
        var certificate = candidates
            .FirstOrDefault(candidate => CertificateSpki(candidate).SequenceEqual(pcpSpki));
        if (certificate is null)
        {
            var pcpDigest = Hex(SHA256.HashData(pcpSpki));
            var candidateDigests = string.Join(
                ",",
                candidates
                    .Select(CertificateSpki)
                    .Where(value => value.Length > 0)
                    .Select(value => Hex(SHA256.HashData(value))));
            throw new InvalidOperationException(
                "Windows did not expose an RSA EK certificate matching the TPM endorsement key " +
                $"(pcp={pcpDigest}, candidates={candidateDigests})");
        }
        var canonicalSpki = CertificateSpki(certificate);
        var chain = new List<byte[]> { certificate.RawData };
        using var builder = new X509Chain();
        builder.ChainPolicy.RevocationMode = X509RevocationMode.NoCheck;
        builder.ChainPolicy.DisableCertificateDownloads = true;
        _ = builder.Build(certificate);
        foreach (var element in builder.ChainElements.Cast<X509ChainElement>())
        {
            if (chain.Count >= 8)
            {
                break;
            }
            if (!chain.Any(existing => existing.SequenceEqual(element.Certificate.RawData)))
            {
                chain.Add(element.Certificate.RawData);
            }
        }
        return new WindowsEkEvidence
        {
            CanonicalSpki = canonicalSpki,
            DeviceKey = Hex(SHA256.HashData(certificate.RawData)),
            Certificate = certificate,
            Chain = chain,
        };
    }

    static byte[] ReadPcpRsaPublicBlob()
    {
        var status = NCryptOpenStorageProvider(out var provider, ProviderName, 0);
        if (status != 0)
        {
            throw new InvalidOperationException(
                $"NCryptOpenStorageProvider failed (0x{unchecked((uint)status):x8})");
        }
        try
        {
            foreach (var property in new[] { "PCP_RSA_EKPUB", "PCP_EKPUB" })
            {
                status = NCryptGetProperty(provider, property, null, 0, out var length, 0);
                if (status != 0 || length <= 0)
                {
                    continue;
                }
                var bytes = new byte[length];
                status = NCryptGetProperty(provider, property, bytes, bytes.Length, out length, 0);
                if (status != 0 || length < 24)
                {
                    continue;
                }
                if (length != bytes.Length)
                {
                    Array.Resize(ref bytes, length);
                }
                if (BinaryPrimitives.ReadUInt32LittleEndian(bytes.AsSpan(0, 4)) == RsaPublicMagic)
                {
                    return bytes;
                }
            }
            throw new InvalidOperationException(
                "Windows Platform Crypto Provider did not expose an RSA TPM EK public key");
        }
        finally
        {
            NCryptFreeObject(provider);
        }
    }

    static RSAParameters RsaParameters(byte[] blob)
    {
        if (blob.Length < 24 ||
            BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(0, 4)) != RsaPublicMagic)
        {
            throw new InvalidOperationException("Windows TPM RSA EK public blob is invalid");
        }
        var bitLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(4, 4)));
        var exponentLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(8, 4)));
        var modulusLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(blob.AsSpan(12, 4)));
        if (bitLength < 2048 || bitLength > 4096 ||
            exponentLength <= 0 || exponentLength > 8 ||
            modulusLength != bitLength / 8 ||
            blob.Length != 24 + exponentLength + modulusLength)
        {
            throw new InvalidOperationException("Windows TPM RSA EK public blob has an invalid length");
        }
        return new RSAParameters
        {
            Exponent = blob.AsSpan(24, exponentLength).ToArray(),
            Modulus = blob.AsSpan(24 + exponentLength, modulusLength).ToArray(),
        };
    }

    static byte[] RsaSpki(RSAParameters parameters)
    {
        using var rsa = RSA.Create();
        rsa.ImportParameters(parameters);
        return rsa.ExportSubjectPublicKeyInfo();
    }

    static byte[] CertificateSpki(X509Certificate2 certificate)
    {
        using var rsa = certificate.GetRSAPublicKey();
        if (rsa is null)
        {
            return Array.Empty<byte>();
        }
        return rsa.ExportSubjectPublicKeyInfo();
    }

    static IEnumerable<X509Certificate2> ReadEkCertificates()
    {
        var found = new Dictionary<string, X509Certificate2>(StringComparer.OrdinalIgnoreCase);
        var suppliedJson = Environment.GetEnvironmentVariable(
            "MAYHEM_TPM2_EK_CERTIFICATES_JSON");
        if (!string.IsNullOrWhiteSpace(suppliedJson))
        {
            if (suppliedJson.Length > 2 * 1024 * 1024)
            {
                throw new InvalidOperationException(
                    "Windows EK certificate input exceeds its bound");
            }
            var supplied = JsonSerializer.Deserialize<string[]>(suppliedJson) ??
                throw new InvalidOperationException("Windows EK certificate input is invalid");
            if (supplied.Length == 0 || supplied.Length > 8)
            {
                throw new InvalidOperationException(
                    "Windows EK certificate input count is invalid");
            }
            foreach (var encoded in supplied)
            {
                byte[] der;
                try
                {
                    der = Convert.FromBase64String(encoded);
                }
                catch (FormatException)
                {
                    throw new InvalidOperationException(
                        "Windows EK certificate input is not base64");
                }
                if (der.Length == 0 || der.Length > 128 * 1024 ||
                    Convert.ToBase64String(der) != encoded)
                {
                    throw new InvalidOperationException(
                        "Windows EK certificate input is outside its bound");
                }
#pragma warning disable SYSLIB0057
                var certificate = new X509Certificate2(der);
#pragma warning restore SYSLIB0057
                found.TryAdd(certificate.Thumbprint, certificate);
            }
        }
        foreach (var property in new[]
        {
            "PCP_RSA_EKCERT",
            "PCP_RSA_EKNVCERT",
            "PCP_EKCERT",
            "PCP_EKNVCERT",
        })
        {
            var encoded = TryReadPcpProperty(property);
            if (encoded is null)
            {
                continue;
            }
            try
            {
                var collection = new X509Certificate2Collection();
#pragma warning disable SYSLIB0057
                collection.Import(encoded);
#pragma warning restore SYSLIB0057
                foreach (var certificate in collection.Cast<X509Certificate2>())
                {
                    found.TryAdd(certificate.Thumbprint, certificate);
                }
            }
            catch (CryptographicException)
            {
            }
        }
        foreach (var storeName in new[] { "EKCertStore", "EKCertStoreECC" })
        {
            try
            {
                using var store = new X509Store(storeName, StoreLocation.LocalMachine);
                store.Open(OpenFlags.ReadOnly | OpenFlags.OpenExistingOnly);
                foreach (var certificate in store.Certificates.Cast<X509Certificate2>())
                {
                    found.TryAdd(certificate.Thumbprint, certificate);
                }
            }
            catch (CryptographicException)
            {
            }
        }
        return found.Values;
    }

    static byte[]? TryReadPcpProperty(string property)
    {
        var status = NCryptOpenStorageProvider(out var provider, ProviderName, 0);
        if (status != 0)
        {
            return null;
        }
        try
        {
            status = NCryptGetProperty(provider, property, null, 0, out var length, 0);
            if (status != 0 || length <= 0 || length > 1024 * 1024)
            {
                return null;
            }
            var encoded = new byte[length];
            status = NCryptGetProperty(
                provider,
                property,
                encoded,
                encoded.Length,
                out length,
                0);
            if (status != 0 || length <= 0)
            {
                return null;
            }
            if (length != encoded.Length)
            {
                Array.Resize(ref encoded, length);
            }
            return encoded;
        }
        finally
        {
            NCryptFreeObject(provider);
        }
    }

    static string Hex(byte[] bytes) => Convert.ToHexString(bytes).ToLowerInvariant();
}

sealed class StableTpmMaterial
{
    public TpmPublic AkPublic { get; }
    public TpmPrivate AkPrivate { get; }
    public byte[] AkName { get; }
    public byte[] EkSpki { get; }
    public byte[] EkCertificate { get; }

    StableTpmMaterial(
        TpmPublic akPublic,
        TpmPrivate akPrivate,
        byte[] akName,
        byte[] ekSpki,
        byte[] ekCertificate)
    {
        AkPublic = akPublic;
        AkPrivate = akPrivate;
        AkName = akName;
        EkSpki = ekSpki;
        EkCertificate = ekCertificate;
    }

    public static StableTpmMaterial LoadOrCreate(
        Tpm2 tpm,
        TpmHandle ekHandle,
        WindowsEkEvidence ek,
        string stateRoot)
    {
        var materialRoot = Path.Combine(stateRoot, "material");
        if (!Directory.Exists(materialRoot))
        {
            var temporary = Path.Combine(
                stateRoot,
                $".material.{Environment.ProcessId}.{Convert.ToHexString(RandomNumberGenerator.GetBytes(8))}.tmp");
            Directory.CreateDirectory(temporary);
            try
            {
                var policy = Program.StartEndorsementPolicy(tpm);
                TpmPrivate privatePart;
                TpmPublic publicPart;
                try
                {
                    var template = new TpmPublic(
                        TpmAlgId.Sha256,
                        ObjectAttr.Sign | ObjectAttr.Restricted |
                        ObjectAttr.FixedParent | ObjectAttr.FixedTPM |
                        ObjectAttr.UserWithAuth | ObjectAttr.SensitiveDataOrigin,
                        null,
                        new RsaParms(
                            new SymDefObject(),
                            new SchemeRsassa(TpmAlgId.Sha256),
                            2048,
                            0),
                        new Tpm2bPublicKeyRsa());
                    privatePart = tpm[policy].Create(
                        ekHandle,
                        new SensitiveCreate(null, null),
                        template,
                        null,
                        Array.Empty<PcrSelection>(),
                        out publicPart,
                        out _,
                        out _,
                        out _);
                }
                finally
                {
                    tpm.FlushContext(policy);
                }
                Program.WriteNew(Path.Combine(temporary, "ak.public"), publicPart.GetTpmRepresentation());
                Program.WriteNew(Path.Combine(temporary, "ak.private"), privatePart.GetTpmRepresentation());
                Program.WriteNew(Path.Combine(temporary, "ak.name"), publicPart.GetName());
                Program.WriteNew(Path.Combine(temporary, "ek.spki.der"), ek.CanonicalSpki);
                Program.WriteNew(Path.Combine(temporary, "ek-cert.der"), ek.Certificate.RawData);
                Program.WriteNew(Path.Combine(temporary, "schema"), Encoding.ASCII.GetBytes("1\n"));
                Directory.Move(temporary, materialRoot);
            }
            catch
            {
                if (Directory.Exists(temporary))
                {
                    Directory.Delete(temporary, true);
                }
                throw;
            }
        }

        Program.RequireOrdinaryDirectory(materialRoot);
        var schema = Program.ReadOrdinaryFile(Path.Combine(materialRoot, "schema"), 16);
        if (Encoding.ASCII.GetString(schema) != "1\n")
        {
            throw new InvalidOperationException("unsupported TPM state schema");
        }
        var akPublicBytes = Program.ReadOrdinaryFile(Path.Combine(materialRoot, "ak.public"), 8192);
        var akPrivateBytes = Program.ReadOrdinaryFile(Path.Combine(materialRoot, "ak.private"), 8192);
        var akName = Program.ReadOrdinaryFile(Path.Combine(materialRoot, "ak.name"), 256);
        var ekSpki = Program.ReadOrdinaryFile(Path.Combine(materialRoot, "ek.spki.der"), 8192);
        var ekCertificate = Program.ReadOrdinaryFile(Path.Combine(materialRoot, "ek-cert.der"), 128 * 1024);
        if (!ekSpki.SequenceEqual(ek.CanonicalSpki) ||
            !ekCertificate.SequenceEqual(ek.Certificate.RawData))
        {
            throw new InvalidOperationException("TPM EK identity changed since provider enrollment");
        }
        var akPublic = Marshaller.FromTpmRepresentation<TpmPublic>(akPublicBytes);
        var akPrivate = Marshaller.FromTpmRepresentation<TpmPrivate>(akPrivateBytes);
        if (!akPublic.GetName().SequenceEqual(akName))
        {
            throw new InvalidOperationException("persisted TPM AK name does not match its public area");
        }
        return new StableTpmMaterial(akPublic, akPrivate, akName, ekSpki, ekCertificate);
    }
}

static class Program
{
    const int MaxInputBytes = 64 * 1024;
    const int MaxOutputBytes = 512 * 1024;
    const int MaxReplayEntries = 256;
    const int MaxChallengeLifetimeSeconds = 300;

    static readonly byte[] StandardEkPolicy =
    {
        0x83, 0x71, 0x97, 0x67, 0x44, 0x84, 0xb3, 0xf8,
        0x1a, 0x90, 0xcc, 0x8d, 0x46, 0xa5, 0xd7, 0x24,
        0xfd, 0x52, 0xd7, 0x6e, 0x06, 0x52, 0x0b, 0x64,
        0xf2, 0xa1, 0xda, 0x1b, 0x33, 0x14, 0x69, 0xaa,
    };

    public static int Main(string[] args)
    {
        try
        {
            if (args.Length != 1 ||
                (args[0] != "quote" && args[0] != "activate_credential"))
            {
                throw new InvalidOperationException(
                    "operation must be quote or activate_credential");
            }
            var stateRoot = NeedEnvironment("MAYHEM_TPM2_STATE_DIR");
            if (!Path.IsPathFullyQualified(stateRoot))
            {
                throw new InvalidOperationException("MAYHEM_TPM2_STATE_DIR must be absolute");
            }
            stateRoot = Path.GetFullPath(stateRoot);
            Directory.CreateDirectory(stateRoot);
            RequireOrdinaryDirectory(stateRoot);

            using var stateLock = AcquireStateLock(Path.Combine(stateRoot, ".helper.lock"));
            var ekEvidence = WindowsEkEvidence.Read();
            using var device = new TbsDevice();
            device.Connect();
            using var tpm = new Tpm2(device);
            TpmHandle? ekHandle = null;
            TpmHandle? akHandle = null;
            try
            {
                ekHandle = CreateEndorsementPrimary(tpm, out var ekPublic);
                var createdSpki = RsaTpmPublicSpki(ekPublic);
                if (!createdSpki.SequenceEqual(ekEvidence.CanonicalSpki))
                {
                    throw new InvalidOperationException(
                        "standard endorsement primary does not match the EK certificate");
                }
                var material = StableTpmMaterial.LoadOrCreate(
                    tpm,
                    ekHandle,
                    ekEvidence,
                    stateRoot);
                var loadPolicy = StartEndorsementPolicy(tpm);
                try
                {
                    akHandle = tpm[loadPolicy].Load(
                        ekHandle,
                        material.AkPrivate,
                        material.AkPublic);
                }
                finally
                {
                    tpm.FlushContext(loadPolicy);
                }
                if (!material.AkPublic.GetName().SequenceEqual(material.AkName))
                {
                    throw new InvalidOperationException("loaded TPM AK identity changed");
                }

                if (args[0] == "quote")
                {
                    RunQuote(tpm, akHandle, material, ekEvidence);
                }
                else
                {
                    RunActivateCredential(tpm, akHandle, ekHandle, material, stateRoot);
                }
            }
            finally
            {
                if (akHandle is not null)
                {
                    try { tpm.FlushContext(akHandle); } catch { }
                }
                if (ekHandle is not null)
                {
                    try { tpm.FlushContext(ekHandle); } catch { }
                }
            }
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine($"mayhem-tpm2-quote-windows: {error.Message}");
            return 1;
        }
    }

    static void RunQuote(
        Tpm2 tpm,
        TpmHandle akHandle,
        StableTpmMaterial material,
        WindowsEkEvidence ek)
    {
        var hardwareBinding = NeedLowerHexDigest("MAYHEM_HW_QUOTE_BINDING");
        var extraData = NeedLowerHexDigest("MAYHEM_TPM2_QUOTE_EXTRA_DATA");
        var pcrSelectionText = Environment.GetEnvironmentVariable("MAYHEM_TPM2_PCR_SELECTION");
        if (string.IsNullOrWhiteSpace(pcrSelectionText))
        {
            pcrSelectionText = "sha256:0,2,4,7";
        }
        var pcrIndices = ParsePcrSelection(pcrSelectionText);
        var selections = new[] { new PcrSelection(TpmAlgId.Sha256, pcrIndices) };
        var quotedInfo = tpm[Auth.Pw].Quote(
            akHandle,
            Convert.FromHexString(extraData),
            new SchemeRsassa(TpmAlgId.Sha256),
            selections,
            out var quoteSignature);
        tpm.PcrRead(selections, out var selectedPcrs, out var pcrValues);
        if (selectedPcrs.Length != 1 || pcrValues.Length != pcrIndices.Length)
        {
            throw new InvalidOperationException("TPM returned an unexpected PCR set");
        }
        if (!material.AkPublic.VerifyQuote(
                TpmAlgId.Sha256,
                selectedPcrs,
                pcrValues,
                Convert.FromHexString(extraData),
                quotedInfo,
                quoteSignature))
        {
            throw new InvalidOperationException("TPM quote failed local verification");
        }

        var pcrOutput = new List<Dictionary<string, object?>>();
        for (var index = 0; index < pcrIndices.Length; index++)
        {
            var digest = pcrValues[index].buffer;
            if (digest.Length != 32)
            {
                throw new InvalidOperationException("TPM returned a non-SHA-256 PCR value");
            }
            pcrOutput.Add(new Dictionary<string, object?>
            {
                ["hash_algorithm"] = "sha256",
                ["index"] = pcrIndices[index],
                ["digest"] = Hex(digest),
            });
        }

        var akNameB64 = Convert.ToBase64String(material.AkName);
        var evidence = new Dictionary<string, object?>
        {
            ["schema_version"] = 1,
            ["ak_public_b64"] = Convert.ToBase64String(material.AkPublic.GetTpm2BRepresentation()),
            ["ak_name_b64"] = akNameB64,
            ["quote_attest_b64"] = Convert.ToBase64String(quotedInfo.GetTpmRepresentation()),
            ["quote_signature_b64"] =
                Convert.ToBase64String(Marshaller.GetTpmRepresentation(quoteSignature)),
            ["pcr_values"] = pcrOutput,
        };
        var hardwareQuote = new Dictionary<string, object?>
        {
            ["kind"] = "tpm2_quote_ek",
            ["evidence"] = JsonSerializer.Serialize(evidence),
            ["binding"] = hardwareBinding,
            ["endorsements"] = ek.Chain.Select(Convert.ToBase64String).ToArray(),
            ["metadata"] = null,
        };
        var output = new Dictionary<string, object?>
        {
            ["hardware_quote"] = hardwareQuote,
            ["device_key"] = ek.DeviceKey,
            ["tpm_activate_credential_hello"] = new Dictionary<string, object?>
            {
                ["schema_version"] = 1,
                ["ek_profile"] = "rsa_sha256_aes128_cfb",
                ["ek_public_spki_der_b64"] = Convert.ToBase64String(material.EkSpki),
                ["ak_name_b64"] = akNameB64,
                ["quote_binding"] = hardwareBinding,
            },
        };
        WriteBoundedJson(output, MaxOutputBytes);
    }

    static void RunActivateCredential(
        Tpm2 tpm,
        TpmHandle akHandle,
        TpmHandle ekHandle,
        StableTpmMaterial material,
        string stateRoot)
    {
        var raw = ReadBoundedInput(MaxInputBytes);
        var challenge = ParseChallenge(raw, material);
        var replayRoot = Path.Combine(stateRoot, "replays");
        Directory.CreateDirectory(replayRoot);
        RequireOrdinaryDirectory(replayRoot);
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        PruneReplayCache(replayRoot, now);
        var replayPath = Path.Combine(replayRoot, challenge.ChallengeId + ".json");
        var requestHash = Hex(SHA256.HashData(raw));
        if (File.Exists(replayPath))
        {
            var cached = ReadReplay(replayPath);
            if (!CryptographicOperations.FixedTimeEquals(
                    Convert.FromHexString(cached.RequestSha256),
                    Convert.FromHexString(requestHash)))
            {
                throw new InvalidOperationException(
                    "TPM activation challenge id was reused with different bytes");
            }
            WriteBoundedText(cached.ResponseJson, 4096);
            return;
        }
        if (Directory.EnumerateFiles(replayRoot, "*.json").Take(MaxReplayEntries + 1).Count() >=
            MaxReplayEntries)
        {
            throw new InvalidOperationException("TPM activation replay cache is full");
        }

        var credentialBlob = UnwrapTpm2b(challenge.CredentialBlob, "credential_blob_b64");
        var encryptedSecret = UnwrapTpm2b(challenge.EncryptedSecret, "encrypted_secret_b64");
        var credential = Marshaller.FromTpmRepresentation<IdObject>(credentialBlob);
        var policy = StartEndorsementPolicy(tpm);
        byte[] activated;
        try
        {
            activated = tpm[Auth.Pw, policy].ActivateCredential(
                akHandle,
                ekHandle,
                credential,
                encryptedSecret);
        }
        finally
        {
            tpm.FlushContext(policy);
        }
        if (activated.Length != 32)
        {
            throw new InvalidOperationException("TPM activated secret must be exactly 32 bytes");
        }
        var response = new Dictionary<string, object?>
        {
            ["schema_version"] = 1,
            ["challenge_id"] = challenge.ChallengeId,
            ["ak_name_b64"] = challenge.AkNameB64,
            ["quote_binding"] = challenge.QuoteBinding,
            ["activated_secret_b64"] = Convert.ToBase64String(activated),
        };
        var responseJson = JsonSerializer.Serialize(response);
        if (Encoding.UTF8.GetByteCount(responseJson) > 4096)
        {
            throw new InvalidOperationException("TPM activation response exceeds 4 KiB");
        }
        var cache = new Dictionary<string, object?>
        {
            ["request_sha256"] = requestHash,
            ["expires_at_unix"] = challenge.ExpiresAtUnix,
            ["response_json"] = responseJson,
        };
        WriteAtomic(replayPath, Encoding.UTF8.GetBytes(JsonSerializer.Serialize(cache)));
        WriteBoundedText(responseJson, 4096);
    }

    sealed record ActivationChallenge(
        string ChallengeId,
        string AkNameB64,
        string QuoteBinding,
        byte[] CredentialBlob,
        byte[] EncryptedSecret,
        long ExpiresAtUnix);

    static ActivationChallenge ParseChallenge(byte[] raw, StableTpmMaterial material)
    {
        using var document = JsonDocument.Parse(
            raw,
            new JsonDocumentOptions
            {
                AllowTrailingCommas = false,
                CommentHandling = JsonCommentHandling.Disallow,
                MaxDepth = 8,
            });
        var root = document.RootElement;
        if (root.ValueKind != JsonValueKind.Object)
        {
            throw new InvalidOperationException("TPM activation challenge must be a JSON object");
        }
        var expected = new HashSet<string>(StringComparer.Ordinal)
        {
            "schema_version",
            "challenge_id",
            "ek_public_sha256",
            "ak_name_b64",
            "quote_binding",
            "credential_blob_b64",
            "encrypted_secret_b64",
            "issued_at_unix",
            "expires_at_unix",
        };
        var seen = new HashSet<string>(StringComparer.Ordinal);
        foreach (var property in root.EnumerateObject())
        {
            if (!expected.Contains(property.Name) || !seen.Add(property.Name))
            {
                throw new InvalidOperationException(
                    $"unexpected or duplicate TPM activation field: {property.Name}");
            }
        }
        if (!seen.SetEquals(expected) ||
            RequiredInteger(root, "schema_version") != 1)
        {
            throw new InvalidOperationException(
                "TPM activation challenge fields do not match schema version 1");
        }

        var challengeId = RequiredDigest(root, "challenge_id");
        var ekPublicSha256 = RequiredDigest(root, "ek_public_sha256");
        var quoteBinding = RequiredDigest(root, "quote_binding");
        var akNameB64 = RequiredString(root, "ak_name_b64", 512);
        var akName = DecodeCanonicalBase64(akNameB64, 256, "ak_name_b64");
        if (!akName.SequenceEqual(material.AkName))
        {
            throw new InvalidOperationException("TPM activation challenge targets a different AK");
        }
        var expectedEk = Hex(SHA256.HashData(material.EkSpki));
        if (!CryptographicOperations.FixedTimeEquals(
                Convert.FromHexString(ekPublicSha256),
                Convert.FromHexString(expectedEk)))
        {
            throw new InvalidOperationException("TPM activation challenge targets a different EK");
        }
        var credentialBlob = DecodeCanonicalBase64(
            RequiredString(root, "credential_blob_b64", 16 * 1024),
            8192,
            "credential_blob_b64");
        var encryptedSecret = DecodeCanonicalBase64(
            RequiredString(root, "encrypted_secret_b64", 16 * 1024),
            8192,
            "encrypted_secret_b64");
        _ = UnwrapTpm2b(credentialBlob, "credential_blob_b64");
        _ = UnwrapTpm2b(encryptedSecret, "encrypted_secret_b64");

        var issued = RequiredInteger(root, "issued_at_unix");
        var expires = RequiredInteger(root, "expires_at_unix");
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        if (issued < 0 || expires <= issued ||
            expires - issued > MaxChallengeLifetimeSeconds ||
            issued > now + 30 ||
            expires < now)
        {
            throw new InvalidOperationException("TPM activation challenge lifetime is invalid");
        }
        return new ActivationChallenge(
            challengeId,
            akNameB64,
            quoteBinding,
            credentialBlob,
            encryptedSecret,
            expires);
    }

    sealed record ReplayRecord(string RequestSha256, long ExpiresAtUnix, string ResponseJson);

    static ReplayRecord ReadReplay(string path)
    {
        var bytes = ReadOrdinaryFile(path, 16 * 1024);
        using var document = JsonDocument.Parse(bytes);
        var root = document.RootElement;
        var expected = new HashSet<string>(StringComparer.Ordinal)
        {
            "request_sha256", "expires_at_unix", "response_json",
        };
        var seen = new HashSet<string>(StringComparer.Ordinal);
        foreach (var property in root.EnumerateObject())
        {
            if (!expected.Contains(property.Name) || !seen.Add(property.Name))
            {
                throw new InvalidOperationException("TPM replay record is invalid");
            }
        }
        if (!seen.SetEquals(expected))
        {
            throw new InvalidOperationException("TPM replay record is incomplete");
        }
        return new ReplayRecord(
            RequiredDigest(root, "request_sha256"),
            RequiredInteger(root, "expires_at_unix"),
            RequiredString(root, "response_json", 4096));
    }

    static void PruneReplayCache(string replayRoot, long now)
    {
        foreach (var path in Directory.EnumerateFiles(replayRoot, "*.json"))
        {
            var record = ReadReplay(path);
            if (record.ExpiresAtUnix < now)
            {
                File.Delete(path);
            }
        }
    }

    static TpmHandle CreateEndorsementPrimary(Tpm2 tpm, out TpmPublic publicArea)
    {
        var template = new TpmPublic(
            TpmAlgId.Sha256,
            ObjectAttr.Restricted | ObjectAttr.Decrypt |
            ObjectAttr.FixedParent | ObjectAttr.FixedTPM |
            ObjectAttr.AdminWithPolicy | ObjectAttr.SensitiveDataOrigin,
            StandardEkPolicy,
            new RsaParms(
                new SymDefObject(TpmAlgId.Aes, 128, TpmAlgId.Cfb),
                null,
                2048,
                0),
            new Tpm2bPublicKeyRsa(new byte[256]));
        return tpm[Auth.Pw].CreatePrimary(
            TpmRh.Endorsement,
            new SensitiveCreate(null, null),
            template,
            null,
            Array.Empty<PcrSelection>(),
            out publicArea,
            out _,
            out _,
            out _);
    }

    public static AuthSession StartEndorsementPolicy(Tpm2 tpm)
    {
        var session = tpm.StartAuthSessionEx(TpmSe.Policy, TpmAlgId.Sha256);
        try
        {
            tpm[Auth.Pw].PolicySecret(
                TpmRh.Endorsement,
                session,
                Array.Empty<byte>(),
                Array.Empty<byte>(),
                Array.Empty<byte>(),
                0,
                out _);
            return session;
        }
        catch
        {
            tpm.FlushContext(session);
            throw;
        }
    }

    static byte[] RsaTpmPublicSpki(TpmPublic publicArea)
    {
        if (publicArea.type != TpmAlgId.Rsa ||
            publicArea.parameters is not RsaParms parameters ||
            publicArea.unique is not Tpm2bPublicKeyRsa unique)
        {
            throw new InvalidOperationException("TPM endorsement primary is not RSA");
        }
        var exponentValue = parameters.exponent == 0 ? 65537u : parameters.exponent;
        var exponent = BitConverter.GetBytes(exponentValue);
        if (BitConverter.IsLittleEndian)
        {
            Array.Reverse(exponent);
        }
        exponent = exponent.SkipWhile(value => value == 0).ToArray();
        using var rsa = RSA.Create();
        rsa.ImportParameters(new RSAParameters
        {
            Modulus = unique.buffer,
            Exponent = exponent,
        });
        return rsa.ExportSubjectPublicKeyInfo();
    }

    static uint[] ParsePcrSelection(string selection)
    {
        if (!selection.StartsWith("sha256:", StringComparison.Ordinal))
        {
            throw new InvalidOperationException(
                "TPM PCR selection must contain exactly one SHA-256 bank");
        }
        var values = selection["sha256:".Length..]
            .Split(',', StringSplitOptions.RemoveEmptyEntries);
        if (values.Length == 0)
        {
            throw new InvalidOperationException("TPM PCR selection is empty");
        }
        var parsed = values.Select(value =>
        {
            if (!uint.TryParse(value, out var index) || index > 23)
            {
                throw new InvalidOperationException("TPM PCR selection contains an invalid index");
            }
            return index;
        }).ToArray();
        if (parsed.Distinct().Count() != parsed.Length)
        {
            throw new InvalidOperationException("TPM PCR selection contains a duplicate index");
        }
        Array.Sort(parsed);
        return parsed;
    }

    static FileStream AcquireStateLock(string path)
    {
        var deadline = DateTime.UtcNow.AddSeconds(30);
        while (true)
        {
            try
            {
                return new FileStream(
                    path,
                    FileMode.OpenOrCreate,
                    FileAccess.ReadWrite,
                    FileShare.None,
                    1,
                    FileOptions.WriteThrough);
            }
            catch (IOException) when (DateTime.UtcNow < deadline)
            {
                Thread.Sleep(50);
            }
        }
    }

    public static void RequireOrdinaryDirectory(string path)
    {
        var info = new DirectoryInfo(path);
        if (!info.Exists || (info.Attributes & FileAttributes.ReparsePoint) != 0)
        {
            throw new InvalidOperationException($"{path} must be an ordinary directory");
        }
    }

    public static byte[] ReadOrdinaryFile(string path, int maximumBytes)
    {
        var info = new FileInfo(path);
        if (!info.Exists ||
            (info.Attributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0 ||
            info.Length < 0 ||
            info.Length > maximumBytes)
        {
            throw new InvalidOperationException($"{path} is not a bounded ordinary file");
        }
        return File.ReadAllBytes(path);
    }

    public static void WriteNew(string path, byte[] bytes)
    {
        using var stream = new FileStream(
            path,
            FileMode.CreateNew,
            FileAccess.Write,
            FileShare.None,
            4096,
            FileOptions.WriteThrough);
        stream.Write(bytes);
        stream.Flush(true);
    }

    static void WriteAtomic(string path, byte[] bytes)
    {
        var temporary = path + "." + Environment.ProcessId + "." +
            Convert.ToHexString(RandomNumberGenerator.GetBytes(8)) + ".tmp";
        WriteNew(temporary, bytes);
        File.Move(temporary, path, false);
    }

    static byte[] ReadBoundedInput(int maximumBytes)
    {
        using var input = Console.OpenStandardInput();
        using var output = new MemoryStream();
        var buffer = new byte[4096];
        while (true)
        {
            var read = input.Read(buffer, 0, buffer.Length);
            if (read == 0)
            {
                break;
            }
            if (output.Length + read > maximumBytes)
            {
                throw new InvalidOperationException(
                    $"TPM activation challenge exceeds {maximumBytes} bytes");
            }
            output.Write(buffer, 0, read);
        }
        if (output.Length == 0)
        {
            throw new InvalidOperationException("TPM activation challenge is empty");
        }
        return output.ToArray();
    }

    static byte[] UnwrapTpm2b(byte[] encoded, string field)
    {
        if (encoded.Length < 2)
        {
            throw new InvalidOperationException($"{field} is not one canonical TPM2B value");
        }
        var length = BinaryPrimitives.ReadUInt16BigEndian(encoded.AsSpan(0, 2));
        if (length != encoded.Length - 2)
        {
            throw new InvalidOperationException($"{field} is not one canonical TPM2B value");
        }
        return encoded.AsSpan(2).ToArray();
    }

    static byte[] DecodeCanonicalBase64(string encoded, int maximumBytes, string field)
    {
        byte[] decoded;
        try
        {
            decoded = Convert.FromBase64String(encoded);
        }
        catch (FormatException)
        {
            throw new InvalidOperationException($"{field} is not canonical base64");
        }
        if (decoded.Length > maximumBytes ||
            Convert.ToBase64String(decoded) != encoded)
        {
            throw new InvalidOperationException($"{field} is not canonical base64");
        }
        return decoded;
    }

    static string RequiredString(JsonElement root, string name, int maximumLength)
    {
        var property = root.GetProperty(name);
        if (property.ValueKind != JsonValueKind.String)
        {
            throw new InvalidOperationException($"{name} must be a string");
        }
        var value = property.GetString()!;
        if (value.Length > maximumLength)
        {
            throw new InvalidOperationException($"{name} exceeds its bound");
        }
        return value;
    }

    static long RequiredInteger(JsonElement root, string name)
    {
        var property = root.GetProperty(name);
        if (property.ValueKind != JsonValueKind.Number ||
            !property.TryGetInt64(out var value))
        {
            throw new InvalidOperationException($"{name} must be an integer");
        }
        return value;
    }

    static string RequiredDigest(JsonElement root, string name)
    {
        var value = RequiredString(root, name, 64);
        if (value.Length != 64 ||
            value.Any(character =>
                !(character is >= '0' and <= '9') &&
                !(character is >= 'a' and <= 'f')))
        {
            throw new InvalidOperationException($"{name} must be lowercase 32-byte hex");
        }
        return value;
    }

    static string NeedEnvironment(string name)
    {
        var value = Environment.GetEnvironmentVariable(name);
        if (string.IsNullOrWhiteSpace(value))
        {
            throw new InvalidOperationException($"{name} is required");
        }
        return value.Trim();
    }

    static string NeedLowerHexDigest(string name)
    {
        var value = NeedEnvironment(name);
        if (value.Length != 64 ||
            value.Any(character =>
                !(character is >= '0' and <= '9') &&
                !(character is >= 'a' and <= 'f')))
        {
            throw new InvalidOperationException($"{name} must be lowercase 32-byte hex");
        }
        return value;
    }

    static string Hex(byte[] bytes) => Convert.ToHexString(bytes).ToLowerInvariant();

    static void WriteBoundedJson(object value, int maximumBytes)
    {
        WriteBoundedText(JsonSerializer.Serialize(value), maximumBytes);
    }

    static void WriteBoundedText(string value, int maximumBytes)
    {
        var bytes = Encoding.UTF8.GetBytes(value);
        if (bytes.Length > maximumBytes)
        {
            throw new InvalidOperationException("TPM helper output exceeds its bound");
        }
        Console.Out.WriteLine(value);
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
$helperExe = Join-Path $outputRoot "MayhemTpm2ProviderHelper.exe"
$stampMatches = (Test-Path $buildStampPath) -and
  ((Get-Content -Raw $buildStampPath).Trim() -eq $sourceHash)

if (-not $stampMatches -or -not (Test-Path $helperExe)) {
  $rootDigest = [Security.Cryptography.SHA256]::Create().ComputeHash(
    [Text.Encoding]::UTF8.GetBytes($helperRoot.ToLowerInvariant())
  )
  $rootHash = -join ($rootDigest | ForEach-Object { $_.ToString("x2") })
  $buildMutex = New-Object Threading.Mutex(
    $false,
    "Local\MayhemTpm2ProviderHelper-$($rootHash.Substring(0,16))"
  )
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
      & $dotnetPath restore $csprojPath --nologo | Out-Null
      if ($LASTEXITCODE -ne 0) {
        Fail "dotnet restore failed for Microsoft.TSS"
      }
      & $dotnetPath build $csprojPath -c Release --no-restore --nologo | Out-Null
      if ($LASTEXITCODE -ne 0 -or -not (Test-Path $helperExe)) {
        Fail "building the Windows TPM provider helper failed"
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

& $helperExe $Operation
if ($LASTEXITCODE -ne 0) {
  Fail "Windows TPM provider helper failed"
}
