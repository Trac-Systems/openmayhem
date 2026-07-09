#!/bin/sh
set -eu

fail() {
  printf '%s\n' "mayhem-tpm2-verify-dotnet: $*" >&2
  exit 1
}

command -v dotnet >/dev/null 2>&1 || fail "missing required command: dotnet"

dotnet_version="$(dotnet --version)"
dotnet_major="${dotnet_version%%.*}"
case "$dotnet_major" in
  ''|*[!0-9]*)
    fail "could not parse dotnet SDK version: $dotnet_version"
    ;;
esac
[ "$dotnet_major" -ge 6 ] || fail ".NET SDK 6 or newer is required; found $dotnet_version"
target_framework="net${dotnet_major}.0"

helper_root="${MAYHEM_TPM2_VERIFIER_HELPER_DIR:-}"
if [ -z "$helper_root" ]; then
  cache_home="${XDG_CACHE_HOME:-${HOME:-/tmp}/.cache}"
  helper_root="$cache_home/mayhem/tpm2-verify-dotnet"
fi
mkdir -p "$helper_root"

csproj="$helper_root/MayhemTpm2Verify.csproj"
program="$helper_root/Program.cs"

cat >"$csproj.tmp" <<XML
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>$target_framework</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Microsoft.TSS" Version="2.1.1" />
  </ItemGroup>
</Project>
XML
if [ ! -f "$csproj" ] || ! cmp -s "$csproj.tmp" "$csproj"; then
  mv "$csproj.tmp" "$csproj"
else
  rm "$csproj.tmp"
fi

cat >"$program.tmp" <<'CS'
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text;
using System.Text.Json;
using Tpm2Lib;

static string Clean(string? value)
{
    return (value ?? string.Empty).Trim().TrimStart('\uFEFF');
}

static string Required(JsonElement element, string name)
{
    if (!element.TryGetProperty(name, out var value) || value.ValueKind != JsonValueKind.String)
    {
        throw new InvalidOperationException($"missing string field {name}");
    }
    var text = Clean(value.GetString());
    if (text.Length == 0)
    {
        throw new InvalidOperationException($"empty string field {name}");
    }
    return text;
}

static string? OptionalString(JsonElement element, string name)
{
    if (!element.TryGetProperty(name, out var value) || value.ValueKind != JsonValueKind.String)
    {
        return null;
    }
    var text = Clean(value.GetString());
    return text.Length == 0 ? null : text;
}

static byte[] B64(JsonElement element, string name)
{
    return Convert.FromBase64String(Required(element, name));
}

static byte[] HexToBytes(string hex)
{
    if (hex.Length != 64)
    {
        throw new InvalidOperationException("expected binding must be a 32-byte hex digest");
    }
    var bytes = new byte[hex.Length / 2];
    for (var i = 0; i < bytes.Length; i++)
    {
        bytes[i] = Convert.ToByte(hex.Substring(i * 2, 2), 16);
    }
    return bytes;
}

static TpmPublic ParseTpmPublic(byte[] bytes)
{
    try
    {
        return Marshaller.FromTpmRepresentation<TpmPublic>(bytes);
    }
    catch
    {
        return Marshaller.FromTpmRepresentation<Tpm2bPublic>(bytes).publicArea;
    }
}

static ISignatureUnion ParseSignature(byte[] bytes)
{
    try
    {
        return Marshaller.FromTpmRepresentation<Signature>(bytes).signature;
    }
    catch
    {
        return Marshaller.FromTpmRepresentation<SignatureRsassa>(bytes);
    }
}

static T[] ParseSequence<T>(byte[] bytes)
{
    var m = new Marshaller(bytes);
    var values = new List<T>();
    while (m.GetGetPos() < bytes.Length)
    {
        values.Add(m.Get<T>());
    }
    return values.ToArray();
}

static byte[] UIntToBigEndian(uint value)
{
    if (value == 0)
    {
        value = 65537;
    }
    var bytes = BitConverter.GetBytes(value);
    if (BitConverter.IsLittleEndian)
    {
        Array.Reverse(bytes);
    }
    return bytes.SkipWhile(b => b == 0).DefaultIfEmpty((byte)0).ToArray();
}

static bool TpmSequencesEqual<T>(T[] left, T[] right)
{
    return Marshaller.GetTpmRepresentation(left).SequenceEqual(Marshaller.GetTpmRepresentation(right));
}

static byte[] TpmName(TpmPublic publicArea)
{
    if (publicArea.nameAlg != TpmAlgId.Sha256)
    {
        throw new InvalidOperationException($"unsupported AK name algorithm {publicArea.nameAlg}");
    }
    var digest = SHA256.HashData(Marshaller.GetTpmRepresentation(publicArea));
    return new byte[] { 0x00, 0x0b }.Concat(digest).ToArray();
}

static void VerifyQuoteManually(
    TpmPublic akPublic,
    PcrSelection[] selections,
    Tpm2bDigest[] pcrValues,
    byte[] bindingBytes,
    Attest quote,
    ISignatureUnion signature)
{
    if (quote.attested is not QuoteInfo quoteInfo)
    {
        throw new InvalidOperationException("TPM attest structure is not quote info");
    }
    if (quote.magic != Generated.Value)
    {
        throw new InvalidOperationException("TPM quote magic is invalid");
    }
    if (!quote.extraData.SequenceEqual(bindingBytes))
    {
        throw new InvalidOperationException("TPM quote nonce does not match Mayhem binding");
    }
    if (!TpmSequencesEqual(quoteInfo.pcrSelect, selections))
    {
        throw new InvalidOperationException("TPM quote PCR selection does not match evidence PCR selection");
    }
    var pcrMarshaller = new Marshaller();
    foreach (var value in pcrValues)
    {
        pcrMarshaller.Put(value.buffer, "");
    }
    var expectedPcrDigest = SHA256.HashData(pcrMarshaller.GetBytes());
    if (!expectedPcrDigest.SequenceEqual(quoteInfo.pcrDigest))
    {
        throw new InvalidOperationException("TPM quote PCR digest does not match evidence PCR values");
    }
    if (signature is not SignatureRsassa rsassa || rsassa.hash != TpmAlgId.Sha256)
    {
        throw new InvalidOperationException("TPM quote signature is not RSASSA-SHA256");
    }
    if (akPublic.unique is not Tpm2bPublicKeyRsa rsaUnique || akPublic.parameters is not RsaParms rsaParms)
    {
        throw new InvalidOperationException("AK public area is not RSA");
    }
    using var rsa = RSA.Create();
    rsa.ImportParameters(new RSAParameters
    {
        Modulus = rsaUnique.buffer,
        Exponent = UIntToBigEndian(rsaParms.exponent)
    });
    if (!rsa.VerifyData(quote.GetTpmRepresentation(), rsassa.sig, HashAlgorithmName.SHA256, RSASignaturePadding.Pkcs1))
    {
        throw new InvalidOperationException("TPM quote signature did not verify against AK public area");
    }
}

static X509Certificate2Collection LoadPemOrDerFile(string path)
{
    var collection = new X509Certificate2Collection();
    var bytes = File.ReadAllBytes(path);
    var text = Encoding.UTF8.GetString(bytes);
    if (text.Contains("BEGIN CERTIFICATE", StringComparison.Ordinal))
    {
        collection.ImportFromPem(text);
    }
    else
    {
        collection.Add(new X509Certificate2(bytes));
    }
    return collection;
}

static X509Certificate2Collection LoadRootBundle()
{
    var rootsPath = Clean(Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_ROOTS"));
    if (rootsPath.Length == 0)
    {
        throw new InvalidOperationException("MAYHEM_TPM2_EK_ROOTS must point to an admin-pinned TPM EK root certificate file or directory");
    }
    var roots = new X509Certificate2Collection();
    if (Directory.Exists(rootsPath))
    {
        foreach (var file in Directory.EnumerateFiles(rootsPath))
        {
            var ext = Path.GetExtension(file).ToLowerInvariant();
            if (ext is ".cer" or ".crt" or ".der" or ".pem")
            {
                roots.AddRange(LoadPemOrDerFile(file));
            }
        }
    }
    else
    {
        roots.AddRange(LoadPemOrDerFile(rootsPath));
    }
    if (roots.Count == 0)
    {
        throw new InvalidOperationException("admin-pinned TPM EK root bundle was empty");
    }
    return roots;
}

static List<X509Certificate2> EvidenceChain(JsonElement evidence, X509Certificate2 leaf)
{
    var chain = new List<X509Certificate2> { leaf };
    if (evidence.TryGetProperty("ek_chain_der_b64", out var array) && array.ValueKind == JsonValueKind.Array)
    {
        foreach (var item in array.EnumerateArray())
        {
            if (item.ValueKind != JsonValueKind.String)
            {
                continue;
            }
            var certB64 = Clean(item.GetString());
            if (certB64.Length == 0)
            {
                continue;
            }
            var cert = new X509Certificate2(Convert.FromBase64String(certB64));
            if (chain.All(existing => !existing.Thumbprint.Equals(cert.Thumbprint, StringComparison.OrdinalIgnoreCase)))
            {
                chain.Add(cert);
            }
        }
    }
    var extraChainPath = Clean(Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_CHAIN"));
    if (extraChainPath.Length > 0)
    {
        var extra = new X509Certificate2Collection();
        if (Directory.Exists(extraChainPath))
        {
            foreach (var file in Directory.EnumerateFiles(extraChainPath))
            {
                var ext = Path.GetExtension(file).ToLowerInvariant();
                if (ext is ".cer" or ".crt" or ".der" or ".pem")
                {
                    extra.AddRange(LoadPemOrDerFile(file));
                }
            }
        }
        else
        {
            extra.AddRange(LoadPemOrDerFile(extraChainPath));
        }
        foreach (var cert in extra)
        {
            if (chain.All(existing => !existing.Thumbprint.Equals(cert.Thumbprint, StringComparison.OrdinalIgnoreCase)))
            {
                chain.Add(cert);
            }
        }
    }
    return chain;
}

static bool SubjectNamesAllowed(IEnumerable<X509Certificate2> certs)
{
    var labels = Clean(Environment.GetEnvironmentVariable("MAYHEM_TPM2_EK_ALLOWED_MANUFACTURERS"));
    if (labels.Length == 0)
    {
        labels = "Advanced Micro Devices,AMD,Intel,Infineon,Nuvoton,STMicroelectronics,Microsoft,Qualcomm";
    }
    var allowed = labels.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
    return certs.Any(cert => allowed.Any(label =>
        cert.Subject.Contains(label, StringComparison.OrdinalIgnoreCase) ||
        cert.Issuer.Contains(label, StringComparison.OrdinalIgnoreCase)));
}

static void VerifyEkChain(JsonElement evidence)
{
    var leaf = new X509Certificate2(B64(evidence, "ek_cert_der_b64"));
    var evidenceChain = EvidenceChain(evidence, leaf);
    var roots = LoadRootBundle();
    using var chain = new X509Chain();
    chain.ChainPolicy.RevocationMode = X509RevocationMode.NoCheck;
    chain.ChainPolicy.TrustMode = X509ChainTrustMode.CustomRootTrust;
    foreach (var root in roots)
    {
        chain.ChainPolicy.CustomTrustStore.Add(root);
    }
    foreach (var cert in evidenceChain.Skip(1))
    {
        chain.ChainPolicy.ExtraStore.Add(cert);
    }
    if (!chain.Build(leaf))
    {
        var reason = string.Join("; ", chain.ChainStatus.Select(status => status.StatusInformation.Trim()).Where(text => text.Length > 0));
        throw new InvalidOperationException($"TPM EK certificate chain did not validate against admin roots: {reason}");
    }
    if (!SubjectNamesAllowed(evidenceChain.Concat(roots.Cast<X509Certificate2>())))
    {
        throw new InvalidOperationException("TPM EK certificate chain did not contain an allowed manufacturer label");
    }
}

static void VerifyWindowsTbsQuote(JsonElement evidence, string expectedBinding)
{
    var bindingBytes = HexToBytes(expectedBinding);
    var akPublic = ParseTpmPublic(B64(evidence, "ak_public_tpm_b64"));
    var akName = B64(evidence, "ak_name_b64");
    if (!akName.SequenceEqual(TpmName(akPublic)))
    {
        throw new InvalidOperationException("AK name does not match AK public area");
    }
    var quote = Marshaller.FromTpmRepresentation<Attest>(B64(evidence, "quote_attest_b64"));
    var signature = ParseSignature(B64(evidence, "quote_signature_b64"));
    var selections = ParseSequence<PcrSelection>(B64(evidence, "pcr_selection_tpm_b64"));
    var values = ParseSequence<Tpm2bDigest>(B64(evidence, "pcr_values_tpm_b64"));
    VerifyQuoteManually(akPublic, selections, values, bindingBytes, quote, signature);
}

static string MetadataDeviceKey(JsonElement quote)
{
    if (!quote.TryGetProperty("metadata", out var metadata) || metadata.ValueKind != JsonValueKind.Object)
    {
        return string.Empty;
    }
    foreach (var name in new[] { "device_key", "ek_fingerprint", "tpm_ek_sha256" })
    {
        var value = OptionalString(metadata, name);
        if (!string.IsNullOrWhiteSpace(value))
        {
            return value.ToLowerInvariant();
        }
    }
    if (metadata.TryGetProperty("tpm", out var tpm) && tpm.ValueKind == JsonValueKind.Object)
    {
        foreach (var name in new[] { "device_key", "ek_fingerprint", "ek_sha256" })
        {
            var value = OptionalString(tpm, name);
            if (!string.IsNullOrWhiteSpace(value))
            {
                return value.ToLowerInvariant();
            }
        }
    }
    return string.Empty;
}

try
{
    var input = Clean(Console.In.ReadToEnd());
    using var doc = JsonDocument.Parse(input);
    var root = doc.RootElement;
    var kind = Required(root, "kind");
    if (kind != "tpm2_quote_ek")
    {
        throw new InvalidOperationException($"unsupported quote kind {kind}");
    }
    var expectedBinding = Required(root, "expected_binding").ToLowerInvariant();
    var quote = root.GetProperty("quote");
    var quoteBinding = Required(quote, "binding").ToLowerInvariant();
    if (quoteBinding != expectedBinding)
    {
        throw new InvalidOperationException("quote binding does not match verifier binding");
    }
    var evidenceText = Clean(Required(quote, "evidence"));
    using var evidenceDoc = JsonDocument.Parse(evidenceText);
    var evidence = evidenceDoc.RootElement;
    if (Required(evidence, "kind") != "tpm2_quote_ek")
    {
        throw new InvalidOperationException("evidence kind is not tpm2_quote_ek");
    }
    if (Required(evidence, "binding").ToLowerInvariant() != expectedBinding)
    {
        throw new InvalidOperationException("evidence binding does not match verifier binding");
    }
    var deviceKey = Required(evidence, "device_key").ToLowerInvariant();
    if (deviceKey.Length != 64 || deviceKey.Any(ch => !Uri.IsHexDigit(ch)))
    {
        throw new InvalidOperationException("TPM EK device_key must be a 32-byte hex digest");
    }
    var metadataDeviceKey = MetadataDeviceKey(quote);
    if (metadataDeviceKey.Length > 0 && metadataDeviceKey != deviceKey)
    {
        throw new InvalidOperationException("quote metadata device_key does not match TPM evidence device_key");
    }

    VerifyEkChain(evidence);
    var tool = OptionalString(evidence, "tool") ?? string.Empty;
    if (tool.Equals("tss.net/windows-tbs", StringComparison.OrdinalIgnoreCase))
    {
        VerifyWindowsTbsQuote(evidence, expectedBinding);
    }
    else
    {
        throw new InvalidOperationException($"unsupported TPM evidence tool {tool}; use tss.net/windows-tbs evidence for this verifier");
    }

    var verdict = new
    {
        ok = true,
        kind = "tpm2_quote_ek",
        binding = expectedBinding,
        att_tier = 2,
        roots = new[] { "tpm2_ek_cert_chain", "tpm_manufacturer_root" },
        device_key = deviceKey
    };
    Console.WriteLine(JsonSerializer.Serialize(verdict));
}
catch (Exception ex)
{
    var verdict = new
    {
        ok = false,
        kind = "tpm2_quote_ek",
        att_tier = 2,
        reason = ex.Message
    };
    Console.WriteLine(JsonSerializer.Serialize(verdict));
    Environment.ExitCode = 0;
}
CS
if [ ! -f "$program" ] || ! cmp -s "$program.tmp" "$program"; then
  mv "$program.tmp" "$program"
else
  rm "$program.tmp"
fi

dotnet restore "$csproj" --nologo >/dev/null
dotnet run --project "$csproj" -c Release --no-restore --nologo
