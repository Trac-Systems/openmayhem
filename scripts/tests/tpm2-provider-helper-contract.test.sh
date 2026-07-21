#!/bin/sh
set -eu

root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
linux_helper="$root/scripts/hardware/mayhem-tpm2-quote-linux.sh"
windows_helper="$root/scripts/hardware/mayhem-tpm2-quote-windows.ps1"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-tpm-helper-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

fail() {
  printf '%s\n' "tpm2-provider-helper-contract: $*" >&2
  exit 1
}

sh -n "$linux_helper"

forbidden='sudo|runas|start-process.{0,24}-verb|icacls|set-acl|registry|reg\.exe|regset|usermod|groupadd|chown|evictcontrol|persistent.{0,16}handle'
if grep -Eini "$forbidden" "$linux_helper" "$windows_helper" >/dev/null; then
  fail "provider helpers contain a forbidden privilege or machine-setup path"
fi
if grep -En 'MAYHEM_HW_QUOTE_NONCE|MAYHEM_TPM2_USE_SUDO' \
  "$linux_helper" "$windows_helper" >/dev/null; then
  fail "provider helpers retain a legacy quote input"
fi
for helper in "$linux_helper" "$windows_helper"; do
  grep -F 'MAYHEM_HW_QUOTE_BINDING' "$helper" >/dev/null ||
    fail "$helper does not consume the outer hardware-quote binding"
  grep -F 'MAYHEM_TPM2_QUOTE_EXTRA_DATA' "$helper" >/dev/null ||
    fail "$helper does not consume the TPM evidence binding"
done
grep -F 'TPM2TOOLS_TCTI="device:/dev/tpmrm0"' "$linux_helper" >/dev/null ||
  fail "Linux helper does not pin the resource-manager device"
grep -F '[ValidateSet("quote", "activate_credential")]' "$windows_helper" >/dev/null ||
  fail "Windows helper does not expose only the two contract operations"
grep -F 'TpmRh.Endorsement' "$windows_helper" >/dev/null ||
  fail "Windows helper does not use the endorsement hierarchy"
grep -F 'ActivateCredential' "$windows_helper" >/dev/null ||
  fail "Windows helper does not implement credential activation"
grep -F 'DeviceKey = Hex(SHA256.HashData(deviceIdentity))' "$windows_helper" >/dev/null ||
  fail "Windows helper device identity is not derived from stable EK public-key material"
grep -F 'Convert.ToBase64String(EncodeQuoteSignature(rsassa))' "$windows_helper" >/dev/null ||
  fail "Windows helper does not emit one complete canonical TPMT_SIGNATURE"
grep -F 'DisableCertificateDownloads = false' "$windows_helper" >/dev/null ||
  fail "Windows helper cannot retrieve a missing vendor-signed EK intermediate"
grep -F 'UrlRetrievalTimeout = TimeSpan.FromSeconds(10)' "$windows_helper" >/dev/null ||
  fail "Windows helper EK issuer retrieval is not bounded"
mutex_line="$(grep -nF '$buildMutex.WaitOne' "$windows_helper" | head -n 1 | cut -d: -f1)"
source_write_line="$(grep -nF 'Set-Content -Path $programPath' "$windows_helper" | head -n 1 | cut -d: -f1)"
[ -n "$mutex_line" ] && [ -n "$source_write_line" ] && [ "$mutex_line" -lt "$source_write_line" ] ||
  fail "Windows helper writes its shared build cache before acquiring the build mutex"

mkdir -p "$tmp/fixture" "$tmp/mock-bin" "$tmp/state"
openssl req \
  -x509 \
  -newkey rsa:2048 \
  -nodes \
  -subj "/CN=Mayhem mocked TPM EK/" \
  -keyout "$tmp/fixture/ek.key" \
  -out "$tmp/fixture/ek.pem" \
  -days 1 >/dev/null 2>&1
openssl x509 \
  -in "$tmp/fixture/ek.pem" \
  -outform DER \
  -out "$tmp/fixture/ek-cert.der"
openssl x509 -in "$tmp/fixture/ek.pem" -pubkey -noout |
  openssl pkey -pubin -outform DER -out "$tmp/fixture/ek.spki.der"

python3 - "$tmp/fixture" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1])
(root / "ek.pub").write_bytes(b"\x00\x08mock-ek!")
(root / "ak.pub").write_bytes(b"\x00\x0bmock-ak-pub")
(root / "ak.priv").write_bytes(b"\x00\x0cmock-ak-priv")
(root / "ak.name").write_bytes(b"\x00\x0b" + bytes(range(32)))
(root / "activated.bin").write_bytes(bytes(range(32)))
PY

cat >"$tmp/mock-bin/tpm-mock" <<'SH'
#!/bin/sh
set -eu

name="${0##*/}"
if [ "$name" = "flock" ]; then
  exit 0
fi

[ "${TPM2TOOLS_TCTI:-}" = "device:/dev/tpmrm0" ] || {
  printf '%s\n' "mock TPM command used the wrong TCTI" >&2
  exit 1
}
printf '%s %s\n' "$name" "$*" >>"$MAYHEM_MOCK_LOG"

case "$name" in
  tpm2_createek)
    context=""
    public=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -c) context="$2"; shift 2 ;;
        -u) public="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    printf '%s' "mock-ek-context" >"$context"
    cp "$MAYHEM_MOCK_FIXTURE/ek.pub" "$public"
    ;;
  tpm2_createak)
    public=""
    private=""
    name_path=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -u) public="$2"; shift 2 ;;
        -r) private="$2"; shift 2 ;;
        -n) name_path="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    count="$(cat "$MAYHEM_MOCK_CREATEAK_COUNT")"
    printf '%s\n' "$((count + 1))" >"$MAYHEM_MOCK_CREATEAK_COUNT"
    cp "$MAYHEM_MOCK_FIXTURE/ak.pub" "$public"
    cp "$MAYHEM_MOCK_FIXTURE/ak.priv" "$private"
    cp "$MAYHEM_MOCK_FIXTURE/ak.name" "$name_path"
    ;;
  tpm2_nvread)
    output=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -o) output="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    cp "$MAYHEM_MOCK_FIXTURE/ek-cert.der" "$output"
    ;;
  tpm2_readpublic)
    context=""
    format=""
    output=""
    name_path=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -c) context="$2"; shift 2 ;;
        -f) format="$2"; shift 2 ;;
        -o) output="$2"; shift 2 ;;
        -n) name_path="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    case "$context" in
      *ak.ctx)
        cp "$MAYHEM_MOCK_FIXTURE/ak.pub" "$output"
        cp "$MAYHEM_MOCK_FIXTURE/ak.name" "$name_path"
        ;;
      *)
        if [ "$format" = "der" ]; then
          cp "$MAYHEM_MOCK_FIXTURE/ek.spki.der" "$output"
        else
          cp "$MAYHEM_MOCK_FIXTURE/ek.pub" "$output"
        fi
        ;;
    esac
    ;;
  tpm2_load)
    context=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -c) context="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    printf '%s' "mock-ak-context" >"$context"
    ;;
  tpm2_quote)
    message=""
    signature=""
    pcrs=""
    qualification=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -m) message="$2"; shift 2 ;;
        -s) signature="$2"; shift 2 ;;
        -o) pcrs="$2"; shift 2 ;;
        -q) qualification="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    printf '%s' "mock-quote-$qualification" >"$message"
    printf '%s' "mock-signature" >"$signature"
    printf '%s' "mock-pcrs" >"$pcrs"
    ;;
  tpm2_pcrread)
    cat <<'PCR'
sha256:
  0 : 0000000000000000000000000000000000000000000000000000000000000000
  2 : 2222222222222222222222222222222222222222222222222222222222222222
  4 : 4444444444444444444444444444444444444444444444444444444444444444
  7 : 7777777777777777777777777777777777777777777777777777777777777777
PCR
    ;;
  tpm2_startauthsession)
    session=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -S) session="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    printf '%s' "mock-policy-session" >"$session"
    ;;
  tpm2_policysecret)
    ;;
  tpm2_activatecredential)
    input=""
    output=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -i) input="$2"; shift 2 ;;
        -o) output="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    cp "$input" "$MAYHEM_MOCK_LAST_ACTIVATION_INPUT"
    cp "$MAYHEM_MOCK_FIXTURE/activated.bin" "$output"
    ;;
  tpm2_flushcontext)
    ;;
  *)
    printf '%s\n' "unexpected mock TPM command: $name" >&2
    exit 1
    ;;
esac
SH
chmod +x "$tmp/mock-bin/tpm-mock"
for name in \
  flock \
  tpm2_activatecredential \
  tpm2_createak \
  tpm2_createek \
  tpm2_flushcontext \
  tpm2_load \
  tpm2_nvread \
  tpm2_pcrread \
  tpm2_policysecret \
  tpm2_quote \
  tpm2_readpublic \
  tpm2_startauthsession
do
  ln -s tpm-mock "$tmp/mock-bin/$name"
done

: >"$tmp/mock.log"
printf '%s\n' "0" >"$tmp/createak.count"
export MAYHEM_MOCK_FIXTURE="$tmp/fixture"
export MAYHEM_MOCK_LOG="$tmp/mock.log"
export MAYHEM_MOCK_CREATEAK_COUNT="$tmp/createak.count"
export MAYHEM_MOCK_LAST_ACTIVATION_INPUT="$tmp/activation.input"

hardware_binding="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
extra_data="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
PATH="$tmp/mock-bin:$PATH" \
MAYHEM_TPM2_STATE_DIR="$tmp/state" \
MAYHEM_HW_QUOTE_BINDING="$hardware_binding" \
MAYHEM_TPM2_QUOTE_EXTRA_DATA="$extra_data" \
  "$linux_helper" quote >"$tmp/quote-1.json"
PATH="$tmp/mock-bin:$PATH" \
MAYHEM_TPM2_STATE_DIR="$tmp/state" \
MAYHEM_HW_QUOTE_BINDING="$hardware_binding" \
MAYHEM_TPM2_QUOTE_EXTRA_DATA="$extra_data" \
  "$linux_helper" quote >"$tmp/quote-2.json"

[ "$(cat "$tmp/createak.count")" = "1" ] ||
  fail "Linux helper recreated its stable AK"

python3 - "$tmp" "$hardware_binding" "$extra_data" <<'PY'
import base64
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

root = Path(sys.argv[1])
hardware_binding = sys.argv[2]
extra_data = sys.argv[3]
first = json.loads((root / "quote-1.json").read_text())
second = json.loads((root / "quote-2.json").read_text())
assert first == second
assert set(first) == {
    "hardware_quote",
    "device_key",
    "tpm_activate_credential_hello",
}
quote = first["hardware_quote"]
assert set(quote) == {"kind", "evidence", "binding", "endorsements", "metadata"}
assert quote["kind"] == "tpm2_quote_ek"
assert quote["binding"] == hardware_binding
assert quote["metadata"] is None
assert len(quote["endorsements"]) == 1
evidence = json.loads(quote["evidence"])
assert set(evidence) == {
    "schema_version",
    "ak_public_b64",
    "ak_name_b64",
    "quote_attest_b64",
    "quote_signature_b64",
    "pcr_values",
}
assert evidence["schema_version"] == 1
assert base64.b64decode(evidence["quote_attest_b64"]) == (
    "mock-quote-" + extra_data
).encode()
assert [item["index"] for item in evidence["pcr_values"]] == [0, 2, 4, 7]
assert all(item["hash_algorithm"] == "sha256" for item in evidence["pcr_values"])
certificate = (root / "fixture/ek-cert.der").read_bytes()
assert first["device_key"] == hashlib.sha256(certificate).hexdigest()
hello = first["tpm_activate_credential_hello"]
assert set(hello) == {
    "schema_version",
    "ek_profile",
    "ek_public_spki_der_b64",
    "ak_name_b64",
    "quote_binding",
}
assert hello["schema_version"] == 1
assert hello["ek_profile"] == "rsa_sha256_aes128_cfb"
assert hello["quote_binding"] == hardware_binding
assert base64.b64decode(hello["ek_public_spki_der_b64"]) == (
    root / "fixture/ek.spki.der"
).read_bytes()
assert base64.b64decode(hello["ak_name_b64"]) == (
    root / "fixture/ak.name"
).read_bytes()
for path in [root / "state", root / "state/material"]:
    assert stat.S_IMODE(path.stat().st_mode) == 0o700
for path in (root / "state/material").iterdir():
    assert path.is_file() and not path.is_symlink()
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
PY

python3 - "$tmp" <<'PY'
import base64
import hashlib
import json
from pathlib import Path
import sys
import time

root = Path(sys.argv[1])
now = int(time.time())
challenge = {
    "schema_version": 1,
    "challenge_id": "11" * 32,
    "ek_public_sha256": hashlib.sha256(
        (root / "fixture/ek.spki.der").read_bytes()
    ).hexdigest(),
    "ak_name_b64": base64.b64encode(
        (root / "fixture/ak.name").read_bytes()
    ).decode(),
    "quote_binding": "aa" * 32,
    "credential_blob_b64": base64.b64encode(b"\x00\x04abcd").decode(),
    "encrypted_secret_b64": base64.b64encode(b"\x00\x03xyz").decode(),
    "issued_at_unix": now,
    "expires_at_unix": now + 300,
}
(root / "challenge.json").write_text(
    json.dumps(challenge, sort_keys=True, separators=(",", ":"))
)
PY

PATH="$tmp/mock-bin:$PATH" \
MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  "$linux_helper" activate_credential \
  <"$tmp/challenge.json" >"$tmp/activation-1.json"
PATH="$tmp/mock-bin:$PATH" \
MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  "$linux_helper" activate_credential \
  <"$tmp/challenge.json" >"$tmp/activation-2.json"

[ "$(grep -c '^tpm2_activatecredential ' "$tmp/mock.log")" = "1" ] ||
  fail "exact TPM activation replay executed twice"
cmp -s "$tmp/activation-1.json" "$tmp/activation-2.json" ||
  fail "exact TPM activation replay changed its response"

python3 - "$tmp" <<'PY'
import base64
import json
from pathlib import Path
import sys

root = Path(sys.argv[1])
response = json.loads((root / "activation-1.json").read_text())
assert set(response) == {
    "schema_version",
    "challenge_id",
    "ak_name_b64",
    "quote_binding",
    "activated_secret_b64",
}
assert response["schema_version"] == 1
assert response["challenge_id"] == "11" * 32
assert base64.b64decode(response["activated_secret_b64"]) == bytes(range(32))
assert (root / "activation.input").read_bytes() == b"\x00\x04abcd\x00\x03xyz"

challenge = json.loads((root / "challenge.json").read_text())
challenge["quote_binding"] = "bb" * 32
(root / "changed-challenge.json").write_text(
    json.dumps(challenge, sort_keys=True, separators=(",", ":"))
)
PY

if PATH="$tmp/mock-bin:$PATH" \
  MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  "$linux_helper" activate_credential \
  <"$tmp/changed-challenge.json" >"$tmp/changed.out" 2>"$tmp/changed.err"
then
  fail "challenge-id reuse with changed bytes was accepted"
fi
grep -F "challenge id was reused with different bytes" "$tmp/changed.err" >/dev/null ||
  fail "changed challenge replay did not fail for the expected reason"

if PATH="$tmp/mock-bin:$PATH" \
  MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  MAYHEM_HW_QUOTE_BINDING="$hardware_binding" \
  MAYHEM_TPM2_QUOTE_EXTRA_DATA="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" \
  "$linux_helper" quote >"$tmp/uppercase.out" 2>"$tmp/uppercase.err"
then
  fail "uppercase quote extra data was accepted"
fi

if PATH="$tmp/mock-bin:$PATH" \
  MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  MAYHEM_TPM2_QUOTE_EXTRA_DATA="$extra_data" \
  "$linux_helper" quote >"$tmp/missing-outer.out" 2>"$tmp/missing-outer.err"
then
  fail "missing outer hardware-quote binding was accepted"
fi
grep -F "MAYHEM_HW_QUOTE_BINDING is required" "$tmp/missing-outer.err" >/dev/null ||
  fail "missing outer hardware-quote binding did not fail explicitly"

if PATH="$tmp/mock-bin:$PATH" \
  MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  MAYHEM_HW_QUOTE_BINDING="BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB" \
  MAYHEM_TPM2_QUOTE_EXTRA_DATA="$extra_data" \
  "$linux_helper" quote >"$tmp/uppercase-outer.out" 2>"$tmp/uppercase-outer.err"
then
  fail "uppercase outer hardware-quote binding was accepted"
fi

if PATH="$tmp/mock-bin:$PATH" \
  MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  MAYHEM_HW_QUOTE_BINDING="$hardware_binding" \
  "$linux_helper" quote >"$tmp/missing-extra.out" 2>"$tmp/missing-extra.err"
then
  fail "missing TPM quote extra data was accepted"
fi
grep -F "MAYHEM_TPM2_QUOTE_EXTRA_DATA is required" "$tmp/missing-extra.err" >/dev/null ||
  fail "missing TPM quote extra data did not fail explicitly"

python3 - "$tmp/oversized.json" <<'PY'
from pathlib import Path
import sys
Path(sys.argv[1]).write_bytes(b"x" * 65537)
PY
if PATH="$tmp/mock-bin:$PATH" \
  MAYHEM_TPM2_STATE_DIR="$tmp/state" \
  "$linux_helper" activate_credential \
  <"$tmp/oversized.json" >"$tmp/oversized.out" 2>"$tmp/oversized.err"
then
  fail "oversized activation input was accepted"
fi
grep -F "exceeds 64 KiB" "$tmp/oversized.err" >/dev/null ||
  fail "oversized activation input did not fail at the input bound"

if command -v dotnet >/dev/null 2>&1; then
  mkdir -p "$tmp/windows-build"
  awk '
    BEGIN { inside=0 }
    /^\$program = @'\''$/ { inside=1; next }
    inside && /^'\''@$/ { exit }
    inside { print }
  ' "$windows_helper" >"$tmp/windows-build/Program.cs"
  cat >"$tmp/windows-build/ParserContract.cs" <<'CS'
using System;
using System.Buffers.Binary;
using System.Linq;
using Tpm2Lib;

static class ParserContract
{
    static byte[] Tpm2b(byte[] value)
    {
        var encoded = new byte[value.Length + 2];
        BinaryPrimitives.WriteUInt16BigEndian(encoded.AsSpan(0, 2), checked((ushort)value.Length));
        value.CopyTo(encoded, 2);
        return encoded;
    }

    public static int Main()
    {
        var integrity = Enumerable.Range(0, 32).Select(value => (byte)value).ToArray();
        var identity = Enumerable.Range(32, 34).Select(value => (byte)value).ToArray();
        var body = Tpm2b(integrity).Concat(Tpm2b(identity)).ToArray();
        var parsed = Program.ParseCredentialBlob(Tpm2b(body));
        if (!parsed.integrityHMAC.SequenceEqual(integrity) ||
            !parsed.encIdentity.SequenceEqual(identity))
        {
            return 1;
        }

        var signature = Program.EncodeQuoteSignature(
            new SignatureRsassa(TpmAlgId.Sha256, new byte[256]));
        if (signature.Length != 262 ||
            signature[0] != 0x00 || signature[1] != 0x14 ||
            signature[2] != 0x00 || signature[3] != 0x0b ||
            signature[4] != 0x01 || signature[5] != 0x00)
        {
            return 3;
        }

        try
        {
            Program.ParseCredentialBlob(Tpm2b(body.Concat(new byte[] { 0 }).ToArray()));
            return 2;
        }
        catch (InvalidOperationException)
        {
            return 0;
        }
    }
}
CS
  dotnet_major="$(dotnet --version | sed 's/\..*//')"
  cat >"$tmp/windows-build/MayhemTpm2ProviderHelper.csproj" <<EOF
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net${dotnet_major}.0</TargetFramework>
    <StartupObject>ParserContract</StartupObject>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Microsoft.TSS" Version="2.1.1" />
  </ItemGroup>
</Project>
EOF
  (
    cd "$tmp/windows-build"
    dotnet restore --nologo >/dev/null
    dotnet build --no-restore --nologo >/dev/null
    dotnet run --no-build --nologo >/dev/null
  )
fi

if command -v pwsh >/dev/null 2>&1; then
  MAYHEM_PS_HELPER="$windows_helper" pwsh -NoProfile -NonInteractive -Command '
    $tokens = $null
    $errors = $null
    [void][Management.Automation.Language.Parser]::ParseFile(
      $env:MAYHEM_PS_HELPER,
      [ref]$tokens,
      [ref]$errors
    )
    if ($errors.Count -ne 0) {
      $errors | ForEach-Object { [Console]::Error.WriteLine($_) }
      exit 1
    }
  '
fi

printf '%s\n' "tpm2 provider helper contract: ok"
