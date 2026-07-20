#!/bin/sh
set -eu

umask 077

fail() {
  printf '%s\n' "mayhem-tpm2-quote-linux: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

[ "$#" -eq 1 ] || fail "usage: mayhem-tpm2-quote-linux.sh quote|activate_credential"
operation="$1"
case "$operation" in
  quote|activate_credential) ;;
  *) fail "operation must be quote or activate_credential" ;;
esac

state_dir="${MAYHEM_TPM2_STATE_DIR:-}"
[ -n "$state_dir" ] || fail "MAYHEM_TPM2_STATE_DIR is required"
case "$state_dir" in
  /*) ;;
  *) fail "MAYHEM_TPM2_STATE_DIR must be an absolute path" ;;
esac

if [ -e "$state_dir" ]; then
  [ -d "$state_dir" ] && [ ! -L "$state_dir" ] ||
    fail "MAYHEM_TPM2_STATE_DIR must be an ordinary directory"
else
  mkdir -p "$state_dir"
fi
chmod 700 "$state_dir"

need cmp
need flock
need mktemp
need openssl
need python3
need tpm2_createak
need tpm2_createek
need tpm2_load
need tpm2_nvread
need tpm2_readpublic

export TPM2TOOLS_TCTI="device:/dev/tpmrm0"

lock_path="$state_dir/.helper.lock"
if [ -e "$lock_path" ]; then
  [ -f "$lock_path" ] && [ ! -L "$lock_path" ] ||
    fail "TPM state lock is not an ordinary file"
else
  (set -C; : >"$lock_path") 2>/dev/null ||
    fail "could not create TPM state lock"
fi
chmod 600 "$lock_path"
exec 9>"$lock_path"
flock -x 9 || fail "could not lock TPM state"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/mayhem-tpm2-provider.XXXXXX")"
stage=""
cleanup() {
  if [ -n "$stage" ] && [ -d "$stage" ]; then
    rm -rf "$stage"
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

material="$state_dir/material"
pcr_selection="${MAYHEM_TPM2_PCR_SELECTION:-sha256:0,2,4,7}"

prepare_endorsement_key() {
  tpm2_createek -Q -G rsa -c "$tmp/ek.ctx" -u "$tmp/ek.pub"
  tpm2_readpublic -Q -c "$tmp/ek.ctx" -f der -o "$tmp/ek.spki.der"
  tpm2_nvread -Q -C o 0x01c00002 -o "$tmp/ek-cert.der"
  [ -s "$tmp/ek-cert.der" ] || fail "TPM RSA EK certificate is unavailable"

  openssl x509 -inform DER -in "$tmp/ek-cert.der" -noout >/dev/null
  openssl x509 -inform DER -in "$tmp/ek-cert.der" -pubkey -noout |
    openssl pkey -pubin -outform DER -out "$tmp/ek-cert.spki.der"
  cmp -s "$tmp/ek.spki.der" "$tmp/ek-cert.spki.der" ||
    fail "TPM endorsement primary does not match the EK certificate"
}

initialize_material() {
  [ ! -e "$material" ] || return 0
  prepare_endorsement_key

  stage="$state_dir/.material.$$.tmp"
  [ ! -e "$stage" ] || fail "temporary TPM state path already exists"
  mkdir "$stage"
  chmod 700 "$stage"

  tpm2_createak \
    -Q \
    -C "$tmp/ek.ctx" \
    -G rsa \
    -g sha256 \
    -s rsassa \
    -u "$stage/ak.pub" \
    -r "$stage/ak.priv" \
    -n "$stage/ak.name"

  cp "$tmp/ek.pub" "$stage/ek.pub"
  cp "$tmp/ek.spki.der" "$stage/ek.spki.der"
  cp "$tmp/ek-cert.der" "$stage/ek-cert.der"
  printf '%s\n' "1" >"$stage/schema"
  chmod 600 "$stage"/*
  mv "$stage" "$material"
  stage=""
}

validate_material() {
  [ -d "$material" ] && [ ! -L "$material" ] ||
    fail "TPM state material is not an ordinary directory"
  for name in schema ek.pub ek.spki.der ek-cert.der ak.pub ak.priv ak.name; do
    path="$material/$name"
    [ -f "$path" ] && [ ! -L "$path" ] ||
      fail "TPM state material is incomplete: $name"
    chmod 600 "$path"
  done
  [ "$(sed -n '1p' "$material/schema")" = "1" ] ||
    fail "unsupported TPM state schema"
}

load_stable_keys() {
  rm -f \
    "$tmp/ek.ctx" \
    "$tmp/ek.pub" \
    "$tmp/ek.spki.der" \
    "$tmp/ek-cert.der" \
    "$tmp/ek-cert.spki.der"
  prepare_endorsement_key
  cmp -s "$tmp/ek.pub" "$material/ek.pub" ||
    fail "TPM endorsement primary changed since provider enrollment"
  cmp -s "$tmp/ek.spki.der" "$material/ek.spki.der" ||
    fail "TPM EK public key changed since provider enrollment"
  cmp -s "$tmp/ek-cert.der" "$material/ek-cert.der" ||
    fail "TPM EK certificate changed since provider enrollment"

  tpm2_load \
    -Q \
    -C "$tmp/ek.ctx" \
    -u "$material/ak.pub" \
    -r "$material/ak.priv" \
    -c "$tmp/ak.ctx"
  tpm2_readpublic \
    -Q \
    -c "$tmp/ak.ctx" \
    -f tss \
    -o "$tmp/ak.loaded.pub" \
    -n "$tmp/ak.loaded.name"
  cmp -s "$tmp/ak.loaded.pub" "$material/ak.pub" ||
    fail "loaded TPM AK public area does not match provider state"
  cmp -s "$tmp/ak.loaded.name" "$material/ak.name" ||
    fail "loaded TPM AK name does not match provider state"
}

initialize_material
validate_material
load_stable_keys

if [ "$operation" = "quote" ]; then
  hardware_binding="${MAYHEM_HW_QUOTE_BINDING:-}"
  [ -n "$hardware_binding" ] || fail "MAYHEM_HW_QUOTE_BINDING is required"
  case "$hardware_binding" in
    *[!0-9a-f]*) fail "MAYHEM_HW_QUOTE_BINDING must be lowercase hex" ;;
  esac
  [ "${#hardware_binding}" -eq 64 ] ||
    fail "MAYHEM_HW_QUOTE_BINDING must be a 32-byte digest"

  extra_data="${MAYHEM_TPM2_QUOTE_EXTRA_DATA:-}"
  [ -n "$extra_data" ] || fail "MAYHEM_TPM2_QUOTE_EXTRA_DATA is required"
  case "$extra_data" in
    *[!0-9a-f]*) fail "MAYHEM_TPM2_QUOTE_EXTRA_DATA must be lowercase hex" ;;
  esac
  [ "${#extra_data}" -eq 64 ] ||
    fail "MAYHEM_TPM2_QUOTE_EXTRA_DATA must be a 32-byte digest"

  tpm2_quote \
    -Q \
    -c "$tmp/ak.ctx" \
    -l "$pcr_selection" \
    -q "$extra_data" \
    -m "$tmp/quote.attest" \
    -s "$tmp/quote.signature" \
    -f tss \
    -o "$tmp/quote.pcrs" \
    -g sha256
  tpm2_pcrread "$pcr_selection" >"$tmp/pcr-values.txt"

  export MAYHEM_TPM2_MATERIAL="$material"
  export MAYHEM_TPM2_OUTPUT="$tmp"
  export MAYHEM_TPM2_PCR_SELECTION="$pcr_selection"
  export MAYHEM_HW_QUOTE_BINDING="$hardware_binding"
  export MAYHEM_TPM2_QUOTE_EXTRA_DATA="$extra_data"
  python3 - <<'PY'
import base64
import hashlib
import json
import os
import re
from pathlib import Path

material = Path(os.environ["MAYHEM_TPM2_MATERIAL"])
output = Path(os.environ["MAYHEM_TPM2_OUTPUT"])
hardware_binding = os.environ["MAYHEM_HW_QUOTE_BINDING"]
selection = os.environ["MAYHEM_TPM2_PCR_SELECTION"]

match = re.fullmatch(r"sha256:([0-9]+(?:,[0-9]+)*)", selection)
if match is None:
    raise SystemExit("TPM PCR selection must contain exactly one SHA-256 bank")
indices = [int(value) for value in match.group(1).split(",")]
if not indices or len(indices) != len(set(indices)) or any(index < 0 or index > 23 for index in indices):
    raise SystemExit("TPM PCR selection contains invalid or duplicate indices")

observed = {}
for line in (output / "pcr-values.txt").read_text(encoding="utf-8").splitlines():
    stripped = line.strip()
    if not stripped or re.fullmatch(r"sha256\s*:", stripped):
        continue
    item = re.fullmatch(r"([0-9]+)\s*:\s*(?:0x)?([0-9a-fA-F]{64})", stripped)
    if item is None:
        raise SystemExit(f"unexpected tpm2_pcrread output: {stripped}")
    index = int(item.group(1))
    if index in observed:
        raise SystemExit("tpm2_pcrread returned a duplicate PCR")
    observed[index] = item.group(2).lower()
if set(observed) != set(indices):
    raise SystemExit("tpm2_pcrread did not return the exact selected PCRs")

def b64(path):
    return base64.b64encode(Path(path).read_bytes()).decode("ascii")

certificate = (material / "ek-cert.der").read_bytes()
spki = (material / "ek.spki.der").read_bytes()
ak_name_b64 = b64(material / "ak.name")
device_key = hashlib.sha256(certificate).hexdigest()
evidence = {
    "schema_version": 1,
    "ak_public_b64": b64(material / "ak.pub"),
    "ak_name_b64": ak_name_b64,
    "quote_attest_b64": b64(output / "quote.attest"),
    "quote_signature_b64": b64(output / "quote.signature"),
    "pcr_values": [
        {
            "hash_algorithm": "sha256",
            "index": index,
            "digest": observed[index],
        }
        for index in sorted(indices)
    ],
}
hardware_quote = {
    "kind": "tpm2_quote_ek",
    "evidence": json.dumps(evidence, sort_keys=True, separators=(",", ":")),
    "binding": hardware_binding,
    "endorsements": [base64.b64encode(certificate).decode("ascii")],
    "metadata": None,
}
result = {
    "hardware_quote": hardware_quote,
    "device_key": device_key,
    "tpm_activate_credential_hello": {
        "schema_version": 1,
        "ek_profile": "rsa_sha256_aes128_cfb",
        "ek_public_spki_der_b64": base64.b64encode(spki).decode("ascii"),
        "ak_name_b64": ak_name_b64,
        "quote_binding": hardware_binding,
    },
}
encoded = json.dumps(result, sort_keys=True, separators=(",", ":"))
if len(encoded.encode("utf-8")) > 512 * 1024:
    raise SystemExit("TPM quote output exceeds 512 KiB")
print(encoded)
PY
  exit 0
fi

need tpm2_activatecredential
need tpm2_flushcontext
need tpm2_policysecret
need tpm2_startauthsession

python3 -c '
import os
import sys
data = sys.stdin.buffer.read(65537)
if not data:
    raise SystemExit("TPM activation challenge is empty")
if len(data) > 65536:
    raise SystemExit("TPM activation challenge exceeds 64 KiB")
path = sys.argv[1]
with open(path, "xb") as handle:
    handle.write(data)
os.chmod(path, 0o600)
' "$tmp/challenge.json"

if [ -e "$state_dir/replays" ]; then
  [ -d "$state_dir/replays" ] && [ ! -L "$state_dir/replays" ] ||
    fail "TPM replay state is not an ordinary directory"
else
  mkdir "$state_dir/replays"
fi
chmod 700 "$state_dir/replays"
export MAYHEM_TPM2_MATERIAL="$material"
export MAYHEM_TPM2_OUTPUT="$tmp"
export MAYHEM_TPM2_REPLAYS="$state_dir/replays"
python3 - <<'PY'
import base64
import binascii
import hashlib
import json
import os
import re
import time
from pathlib import Path

material = Path(os.environ["MAYHEM_TPM2_MATERIAL"])
output = Path(os.environ["MAYHEM_TPM2_OUTPUT"])
replays = Path(os.environ["MAYHEM_TPM2_REPLAYS"])
raw = (output / "challenge.json").read_bytes()

def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate field: {key}")
        result[key] = value
    return result

try:
    challenge = json.loads(raw, object_pairs_hook=unique_object)
except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
    raise SystemExit(f"invalid TPM activation challenge JSON: {error}")

fields = {
    "schema_version",
    "challenge_id",
    "ek_public_sha256",
    "ak_name_b64",
    "quote_binding",
    "credential_blob_b64",
    "encrypted_secret_b64",
    "issued_at_unix",
    "expires_at_unix",
}
if not isinstance(challenge, dict) or set(challenge) != fields:
    raise SystemExit("TPM activation challenge fields do not match schema version 1")
if challenge["schema_version"] != 1 or isinstance(challenge["schema_version"], bool):
    raise SystemExit("unsupported TPM activation challenge schema")

for field in ("challenge_id", "ek_public_sha256", "quote_binding"):
    value = challenge[field]
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise SystemExit(f"{field} must be lowercase 32-byte hex")

def decode_b64(field, maximum):
    value = challenge[field]
    if not isinstance(value, str) or len(value) > maximum * 2:
        raise SystemExit(f"{field} is invalid")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error):
        raise SystemExit(f"{field} is not canonical base64")
    if len(decoded) > maximum or base64.b64encode(decoded).decode("ascii") != value:
        raise SystemExit(f"{field} is not canonical base64")
    return decoded

ak_name = decode_b64("ak_name_b64", 256)
credential_blob = decode_b64("credential_blob_b64", 8192)
encrypted_secret = decode_b64("encrypted_secret_b64", 8192)
if len(ak_name) != 34 or ak_name[:2] != b"\x00\x0b":
    raise SystemExit("TPM AK name is invalid")
if ak_name != (material / "ak.name").read_bytes():
    raise SystemExit("TPM activation challenge targets a different AK")
spki_digest = hashlib.sha256((material / "ek.spki.der").read_bytes()).hexdigest()
if challenge["ek_public_sha256"] != spki_digest:
    raise SystemExit("TPM activation challenge targets a different EK")

def validate_tpm2b(name, value):
    if len(value) < 2 or int.from_bytes(value[:2], "big") != len(value) - 2:
        raise SystemExit(f"{name} is not one canonical TPM2B value")

validate_tpm2b("credential_blob_b64", credential_blob)
validate_tpm2b("encrypted_secret_b64", encrypted_secret)

issued = challenge["issued_at_unix"]
expires = challenge["expires_at_unix"]
if (
    not isinstance(issued, int)
    or isinstance(issued, bool)
    or not isinstance(expires, int)
    or isinstance(expires, bool)
    or issued < 0
    or expires <= issued
    or expires - issued > 300
):
    raise SystemExit("TPM activation challenge lifetime is invalid")
now = int(time.time())
if issued > now + 30:
    raise SystemExit("TPM activation challenge was issued in the future")
if expires < now:
    raise SystemExit("TPM activation challenge expired")

request_sha256 = hashlib.sha256(raw).hexdigest()
cache_path = replays / f"{challenge['challenge_id']}.json"
for candidate in replays.glob("*.json"):
    if not candidate.is_file() or candidate.is_symlink():
        raise SystemExit(f"invalid TPM activation replay path: {candidate.name}")
    try:
        cached = json.loads(candidate.read_text(encoding="utf-8"))
        if int(cached["expires_at_unix"]) < now:
            candidate.unlink()
    except (OSError, ValueError, KeyError, json.JSONDecodeError):
        raise SystemExit(f"invalid TPM activation replay record: {candidate.name}")

if cache_path.exists():
    if not cache_path.is_file() or cache_path.is_symlink():
        raise SystemExit("TPM activation replay path is not an ordinary file")
    cached = json.loads(cache_path.read_text(encoding="utf-8"))
    if cached.get("request_sha256") != request_sha256:
        raise SystemExit("TPM activation challenge id was reused with different bytes")
    response = cached.get("response")
    if not isinstance(response, dict):
        raise SystemExit("TPM activation replay response is invalid")
    encoded = json.dumps(response, sort_keys=True, separators=(",", ":"))
    (output / "response.json").write_text(encoded + "\n", encoding="utf-8")
    (output / "mode").write_text("cached\n", encoding="ascii")
    raise SystemExit(0)

if len(list(replays.glob("*.json"))) >= 256:
    raise SystemExit("TPM activation replay cache is full")

(output / "activation.input").write_bytes(credential_blob + encrypted_secret)
context = {
    "challenge_id": challenge["challenge_id"],
    "ak_name_b64": challenge["ak_name_b64"],
    "quote_binding": challenge["quote_binding"],
    "request_sha256": request_sha256,
    "expires_at_unix": expires,
}
(output / "activation-context.json").write_text(
    json.dumps(context, sort_keys=True, separators=(",", ":")),
    encoding="utf-8",
)
(output / "mode").write_text("execute\n", encoding="ascii")
PY

if [ "$(sed -n '1p' "$tmp/mode")" = "cached" ]; then
  cat "$tmp/response.json"
  exit 0
fi

tpm2_startauthsession -Q --policy-session -S "$tmp/endorsement-policy.ctx"
tpm2_policysecret -Q -S "$tmp/endorsement-policy.ctx" -c e
tpm2_activatecredential \
  -Q \
  -c "$tmp/ak.ctx" \
  -C "$tmp/ek.ctx" \
  -i "$tmp/activation.input" \
  -o "$tmp/activated-secret.bin" \
  -P "session:$tmp/endorsement-policy.ctx"
tpm2_flushcontext -Q "$tmp/endorsement-policy.ctx"

export MAYHEM_TPM2_MATERIAL="$material"
export MAYHEM_TPM2_OUTPUT="$tmp"
export MAYHEM_TPM2_REPLAYS="$state_dir/replays"
python3 - <<'PY'
import base64
import json
import os
from pathlib import Path

output = Path(os.environ["MAYHEM_TPM2_OUTPUT"])
replays = Path(os.environ["MAYHEM_TPM2_REPLAYS"])
context = json.loads((output / "activation-context.json").read_text(encoding="utf-8"))
secret = (output / "activated-secret.bin").read_bytes()
if len(secret) != 32:
    raise SystemExit("TPM activated secret must be exactly 32 bytes")
response = {
    "schema_version": 1,
    "challenge_id": context["challenge_id"],
    "ak_name_b64": context["ak_name_b64"],
    "quote_binding": context["quote_binding"],
    "activated_secret_b64": base64.b64encode(secret).decode("ascii"),
}
encoded = json.dumps(response, sort_keys=True, separators=(",", ":"))
if len(encoded.encode("utf-8")) > 4096:
    raise SystemExit("TPM activation response exceeds 4 KiB")
cache = {
    "request_sha256": context["request_sha256"],
    "expires_at_unix": context["expires_at_unix"],
    "response": response,
}
cache_path = replays / f"{context['challenge_id']}.json"
temporary = replays / f".{context['challenge_id']}.{os.getpid()}.tmp"
with open(temporary, "x", encoding="utf-8") as handle:
    json.dump(cache, handle, sort_keys=True, separators=(",", ":"))
    handle.flush()
    os.fsync(handle.fileno())
os.chmod(temporary, 0o600)
os.replace(temporary, cache_path)
print(encoded)
PY
