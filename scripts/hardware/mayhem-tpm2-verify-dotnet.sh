#!/bin/sh
set -eu

fail() {
  printf '%s\n' "mayhem-tpm2-verify-dotnet: $*" >&2
  exit 1
}

if ! command -v dotnet >/dev/null 2>&1; then
  if [ -n "${DOTNET_ROOT:-}" ] && [ -x "$DOTNET_ROOT/dotnet" ]; then
    PATH="$DOTNET_ROOT:$PATH"
    export DOTNET_ROOT PATH
  elif [ -n "${HOME:-}" ] && [ -x "$HOME/.dotnet/dotnet" ]; then
    DOTNET_ROOT="$HOME/.dotnet"
    PATH="$DOTNET_ROOT:$PATH"
    export DOTNET_ROOT PATH
  fi
fi
command -v dotnet >/dev/null 2>&1 || fail "missing required command: dotnet"
command -v sha256sum >/dev/null 2>&1 || fail "missing required command: sha256sum"

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

csproj_tmp="$(mktemp "$helper_root/MayhemTpm2Verify.csproj.XXXXXX")"
program_tmp="$(mktemp "$helper_root/Program.cs.XXXXXX")"

cat >"$csproj_tmp" <<XML
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
if [ ! -f "$csproj" ] || ! cmp -s "$csproj_tmp" "$csproj"; then
  mv "$csproj_tmp" "$csproj"
else
  rm -f "$csproj_tmp"
fi

cat >"$program_tmp" <<'CS'
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

static bool EnvFlag(string name)
{
    var value = Clean(Environment.GetEnvironmentVariable(name));
    return value.Equals("1", StringComparison.OrdinalIgnoreCase) ||
        value.Equals("true", StringComparison.OrdinalIgnoreCase) ||
        value.Equals("yes", StringComparison.OrdinalIgnoreCase);
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

static ushort ReadLe16(byte[] bytes, int offset)
{
    if (offset < 0 || offset + 2 > bytes.Length)
    {
        throw new InvalidOperationException("truncated little-endian uint16");
    }
    return (ushort)(bytes[offset] | (bytes[offset + 1] << 8));
}

static uint ReadLe32(byte[] bytes, int offset)
{
    if (offset < 0 || offset + 4 > bytes.Length)
    {
        throw new InvalidOperationException("truncated little-endian uint32");
    }
    return (uint)(bytes[offset] |
        (bytes[offset + 1] << 8) |
        (bytes[offset + 2] << 16) |
        (bytes[offset + 3] << 24));
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

static void VerifyDeviceKeyBinding(JsonElement evidence, string deviceKey)
{
    if (evidence.TryGetProperty("ek_public_tss_b64", out var ekPublicValue) &&
        ekPublicValue.ValueKind == JsonValueKind.String)
    {
        var ekPublic = Convert.FromBase64String(Required(evidence, "ek_public_tss_b64"));
        var expected = Convert.ToHexString(SHA256.HashData(ekPublic)).ToLowerInvariant();
        if (!expected.Equals(deviceKey, StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidOperationException("TPM EK device_key does not match EK public evidence");
        }
        return;
    }

    if (evidence.TryGetProperty("ek_public_bcrypt_b64", out var windowsEkValue) &&
        windowsEkValue.ValueKind == JsonValueKind.String)
    {
        var windowsEk = Convert.FromBase64String(Required(evidence, "ek_public_bcrypt_b64"));
        var identity = WindowsEkIdentityBytes(windowsEk);
        var expected = Convert.ToHexString(SHA256.HashData(identity)).ToLowerInvariant();
        if (!expected.Equals(deviceKey, StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidOperationException("TPM EK device_key does not match Windows PCP EK public evidence");
        }
    }
}

static bool IsWindowsEccPublicMagic(uint magic) =>
    magic is 0x314b4345 or 0x334b4345 or 0x354b4345 or
             0x31534345 or 0x33534345 or 0x35534345;

static byte[] WindowsEkIdentityBytes(byte[] blob)
{
    const uint RsaPublicMagic = 0x31415352;
    if (blob.Length < 8)
    {
        throw new InvalidOperationException("Windows PCP EK public blob is truncated");
    }
    var magic = ReadLe32(blob, 0);
    if (magic == RsaPublicMagic)
    {
        var parameters = WindowsRsaParameters(blob);
        using var rsa = RSA.Create();
        rsa.ImportParameters(parameters);
        return rsa.ExportRSAPublicKey();
    }
    if (IsWindowsEccPublicMagic(magic))
    {
        var keyLength = checked((int)ReadLe32(blob, 4));
        if (keyLength <= 0 || blob.Length != 8 + (keyLength * 2))
        {
            throw new InvalidOperationException("Windows PCP ECC EK public blob has an invalid length");
        }
        return new byte[] { 0x04 }
            .Concat(blob.Skip(8).Take(keyLength))
            .Concat(blob.Skip(8 + keyLength).Take(keyLength))
            .ToArray();
    }
    throw new InvalidOperationException($"unsupported Windows PCP EK public blob magic 0x{magic:x8}");
}

static RSAParameters WindowsRsaParameters(byte[] blob)
{
    if (blob.Length < 24)
    {
        throw new InvalidOperationException("Windows PCP RSA EK public blob is truncated");
    }
    var exponentLength = checked((int)ReadLe32(blob, 8));
    var modulusLength = checked((int)ReadLe32(blob, 12));
    if (exponentLength <= 0 || modulusLength <= 0 || blob.Length != 24 + exponentLength + modulusLength)
    {
        throw new InvalidOperationException("Windows PCP RSA EK public blob has an invalid length");
    }
    return new RSAParameters
    {
        Exponent = blob.Skip(24).Take(exponentLength).ToArray(),
        Modulus = blob.Skip(24 + exponentLength).Take(modulusLength).ToArray(),
    };
}

static bool WindowsEkMatchesCertificate(byte[] blob, X509Certificate2 certificate)
{
    const uint RsaPublicMagic = 0x31415352;
    var magic = ReadLe32(blob, 0);
    if (magic == RsaPublicMagic)
    {
        using var rsa = certificate.GetRSAPublicKey();
        if (rsa is null)
        {
            return false;
        }
        var expected = WindowsRsaParameters(blob);
        var actual = rsa.ExportParameters(false);
        return actual.Exponent.AsSpan().SequenceEqual(expected.Exponent) &&
               actual.Modulus.AsSpan().SequenceEqual(expected.Modulus);
    }
    if (IsWindowsEccPublicMagic(magic))
    {
        using var ecdsa = certificate.GetECDsaPublicKey();
        if (ecdsa is null)
        {
            return false;
        }
        var keyLength = checked((int)ReadLe32(blob, 4));
        if (keyLength <= 0 || blob.Length != 8 + (keyLength * 2))
        {
            return false;
        }
        var actual = ecdsa.ExportParameters(false);
        return actual.Q.X.AsSpan().SequenceEqual(blob.AsSpan(8, keyLength)) &&
               actual.Q.Y.AsSpan().SequenceEqual(blob.AsSpan(8 + keyLength, keyLength));
    }
    return false;
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
    var tool = OptionalString(evidence, "tool") ?? string.Empty;
    if (tool.Equals("tss.net/windows-tbs", StringComparison.OrdinalIgnoreCase))
    {
        var deviceKey = Required(evidence, "device_key").ToLowerInvariant();
        var certificateKey = Convert.ToHexString(SHA256.HashData(leaf.GetPublicKey())).ToLowerInvariant();
        if (!certificateKey.Equals(deviceKey, StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidOperationException("Windows TPM EK certificate public key does not match device_key");
        }
        var publicBlob = B64(evidence, "ek_public_bcrypt_b64");
        if (!WindowsEkMatchesCertificate(publicBlob, leaf))
        {
            throw new InvalidOperationException("Windows PCP EK public evidence does not match TPM EK certificate");
        }
    }
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

static bool VerifyEkRoot(JsonElement evidence)
{
    if (evidence.TryGetProperty("ek_cert_der_b64", out var cert) &&
        cert.ValueKind == JsonValueKind.String &&
        Clean(cert.GetString()).Length > 0)
    {
        VerifyEkChain(evidence);
        return false;
    }
    var tool = OptionalString(evidence, "tool") ?? string.Empty;
    if (EnvFlag("MAYHEM_TPM2_ALLOW_MICROSOFT_VTPM_TEST_ROOT") &&
        tool.Equals("tpm2-tools", StringComparison.OrdinalIgnoreCase))
    {
        return true;
    }
    throw new InvalidOperationException("TPM EK certificate is missing; set MAYHEM_TPM2_EK_ROOTS for production EK-chain verification or MAYHEM_TPM2_ALLOW_MICROSOFT_VTPM_TEST_ROOT=1 for the documented Azure vTPM transport test");
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

static (PcrSelection[] Selections, int SelectedCount) ParseTpm2ToolsPcrSelection(string selectionText)
{
    var parts = selectionText.Split(':', 2, StringSplitOptions.TrimEntries);
    if (parts.Length != 2 || !parts[0].Equals("sha256", StringComparison.OrdinalIgnoreCase))
    {
        throw new InvalidOperationException($"unsupported tpm2-tools PCR selection {selectionText}");
    }
    var pcrs = parts[1]
        .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
        .Select(text =>
        {
            if (!int.TryParse(text, out var pcr) || pcr < 0 || pcr > 23)
            {
                throw new InvalidOperationException($"unsupported tpm2-tools PCR index {text}");
            }
            return pcr;
        })
        .Distinct()
        .OrderBy(pcr => pcr)
        .ToArray();
    if (pcrs.Length == 0)
    {
        throw new InvalidOperationException("tpm2-tools PCR selection must include at least one PCR");
    }
    var selectSize = Math.Max(3, (pcrs.Max() / 8) + 1);
    var select = new byte[selectSize];
    foreach (var pcr in pcrs)
    {
        select[pcr / 8] |= (byte)(1 << (pcr % 8));
    }
    return (new[] { new PcrSelection(TpmAlgId.Sha256, select) }, pcrs.Length);
}

static void VerifyTpm2ToolsPcrHeader(byte[] pcrBlob, PcrSelection[] selections)
{
    if (pcrBlob.Length < 10 || selections.Length != 1)
    {
        throw new InvalidOperationException("tpm2-tools PCR blob is truncated");
    }
    var selectionCount = ReadLe32(pcrBlob, 0);
    var hashAlg = ReadLe16(pcrBlob, 4);
    var selectSize = pcrBlob[6];
    if (selectionCount < 1 || hashAlg != (ushort)TpmAlgId.Sha256)
    {
        throw new InvalidOperationException("tpm2-tools PCR blob does not start with a SHA-256 PCR selection");
    }
    if (selectSize != selections[0].pcrSelect.Length || 7 + selectSize > pcrBlob.Length)
    {
        throw new InvalidOperationException("tpm2-tools PCR blob selection size does not match evidence selection");
    }
    for (var i = 0; i < selectSize; i++)
    {
        if (pcrBlob[7 + i] != selections[0].pcrSelect[i])
        {
            throw new InvalidOperationException("tpm2-tools PCR blob selection does not match evidence selection");
        }
    }
}

static Tpm2bDigest[] ParseTpm2ToolsPcrValues(byte[] pcrBlob, int selectedCount)
{
    const int DigestBufferSize = 64;
    const int DigestRecordSize = 2 + DigestBufferSize;
    for (var offset = 10; offset + 4 + (selectedCount * DigestRecordSize) <= pcrBlob.Length; offset++)
    {
        if (ReadLe32(pcrBlob, offset) != selectedCount)
        {
            continue;
        }
        var pos = offset + 4;
        var values = new List<Tpm2bDigest>();
        var ok = true;
        for (var i = 0; i < selectedCount; i++)
        {
            var length = ReadLe16(pcrBlob, pos);
            if (length != SHA256.HashSizeInBytes || pos + 2 + length > pcrBlob.Length)
            {
                ok = false;
                break;
            }
            values.Add(new Tpm2bDigest(pcrBlob.Skip(pos + 2).Take(length).ToArray()));
            pos += DigestRecordSize;
        }
        if (ok)
        {
            return values.ToArray();
        }
    }
    throw new InvalidOperationException("could not parse tpm2-tools PCR values");
}

static void VerifyTpm2ToolsQuote(JsonElement evidence, string expectedBinding)
{
    var bindingBytes = HexToBytes(expectedBinding);
    var akPublic = ParseTpmPublic(B64(evidence, "ak_public_tss_b64"));
    var akName = B64(evidence, "ak_name_b64");
    if (!akName.SequenceEqual(TpmName(akPublic)))
    {
        throw new InvalidOperationException("AK name does not match AK public area");
    }
    var quote = Marshaller.FromTpmRepresentation<Attest>(B64(evidence, "quote_message_b64"));
    var signature = ParseSignature(B64(evidence, "quote_signature_b64"));
    var (selections, selectedCount) = ParseTpm2ToolsPcrSelection(Required(evidence, "pcr_selection"));
    var pcrBlob = B64(evidence, "quote_pcrs_b64");
    VerifyTpm2ToolsPcrHeader(pcrBlob, selections);
    var values = ParseTpm2ToolsPcrValues(pcrBlob, selectedCount);
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

var verdictBinding = Clean(Environment.GetEnvironmentVariable("MAYHEM_HW_VERIFY_BINDING")).ToLowerInvariant();
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
    verdictBinding = expectedBinding;
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
    VerifyDeviceKeyBinding(evidence, deviceKey);

    var usedMicrosoftVtpmTestRoot = VerifyEkRoot(evidence);
    var tool = OptionalString(evidence, "tool") ?? string.Empty;
    if (tool.Equals("tss.net/windows-tbs", StringComparison.OrdinalIgnoreCase))
    {
        VerifyWindowsTbsQuote(evidence, expectedBinding);
    }
    else if (tool.Equals("tpm2-tools", StringComparison.OrdinalIgnoreCase))
    {
        VerifyTpm2ToolsQuote(evidence, expectedBinding);
    }
    else
    {
        throw new InvalidOperationException($"unsupported TPM evidence tool {tool}; use tss.net/windows-tbs or tpm2-tools evidence for this verifier");
    }

    var roots = usedMicrosoftVtpmTestRoot
        ? new[] { "tpm_manufacturer_root", "microsoft_vtpm_test_root_allowance" }
        : new[] { "tpm2_ek_cert_chain", "tpm_manufacturer_root" };
    var verdict = new
    {
        ok = true,
        kind = "tpm2_quote_ek",
        binding = expectedBinding,
        att_tier = 2,
        roots,
        test_root_allowance = usedMicrosoftVtpmTestRoot,
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
        binding = verdictBinding,
        att_tier = 2,
        reason = ex.Message
    };
    Console.WriteLine(JsonSerializer.Serialize(verdict));
    Environment.ExitCode = 0;
}
CS
if [ ! -f "$program" ] || ! cmp -s "$program_tmp" "$program"; then
  mv "$program_tmp" "$program"
else
  rm -f "$program_tmp"
fi

source_hash="$({ cat "$csproj"; printf '\n--PROGRAM--\n'; cat "$program"; } | sha256sum | awk '{print $1}')"
build_stamp="$helper_root/build-source.sha256"
helper="$helper_root/bin/Release/$target_framework/MayhemTpm2Verify"

lock_dir=""
if command -v flock >/dev/null 2>&1; then
  exec 9>"$helper_root/build.lock"
  flock -w 600 9 || fail "timed out waiting for another TPM verifier build"
else
  lock_dir="$helper_root/build.lock.d"
  attempts=0
  while ! mkdir "$lock_dir" 2>/dev/null; do
    owner_pid="$(cat "$lock_dir/owner-pid" 2>/dev/null || true)"
    if [ -n "$owner_pid" ] && ! kill -0 "$owner_pid" 2>/dev/null; then
      rm -f "$lock_dir/owner-pid"
      rmdir "$lock_dir" 2>/dev/null || true
      continue
    fi
    attempts=$((attempts + 1))
    [ "$attempts" -lt 600 ] || fail "timed out waiting for another TPM verifier build"
    sleep 1
  done
  printf '%s\n' "$$" >"$lock_dir/owner-pid"
  trap 'rm -f "$lock_dir/owner-pid"; rmdir "$lock_dir" 2>/dev/null || true' EXIT HUP INT TERM
fi
stamp_hash=""
if [ -f "$build_stamp" ]; then
  stamp_hash="$(tr -d '\r\n' <"$build_stamp")"
fi
if [ "$stamp_hash" != "$source_hash" ] || [ ! -x "$helper" ]; then
  dotnet restore "$csproj" --nologo >/dev/null
  dotnet build "$csproj" -c Release --no-restore --nologo >/dev/null
  [ -x "$helper" ] || fail "building the TPM verifier helper failed"
  stamp_tmp="$(mktemp "$helper_root/build-source.sha256.XXXXXX")"
  printf '%s\n' "$source_hash" >"$stamp_tmp"
  mv "$stamp_tmp" "$build_stamp"
fi
if [ -n "$lock_dir" ]; then
  rm -f "$lock_dir/owner-pid"
  rmdir "$lock_dir" 2>/dev/null || true
  trap - EXIT HUP INT TERM
else
  flock -u 9
fi

exec "$helper"
