#!/bin/sh
set -eu

input_file="$(mktemp "${TMPDIR:-/tmp}/mayhem-azure-ncc-tier3-verify.XXXXXX")"
cleanup() {
  rm -f "$input_file"
}
trap cleanup EXIT INT TERM
cat >"$input_file"
MAYHEM_AZURE_NCC_VERIFY_INPUT_FILE="$input_file" python3 - "$@" <<'PY'
import base64
import hashlib
import json
import os
import re
import shlex
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request

verdict_binding = (os.environ.get("MAYHEM_HW_VERIFY_BINDING") or "").lower()

def fail(reason):
    print(json.dumps({
        "ok": False,
        "kind": "nvidia_nvtrust_offline_jwt",
        "binding": verdict_binding,
        "att_tier": 3,
        "reason": reason,
    }, sort_keys=True))
    sys.exit(0)

def jstr(value):
    if value is None:
        return None
    return str(value)

def b64url_decode(value):
    return base64.urlsafe_b64decode(value + "=" * ((4 - len(value) % 4) % 4))

def der_len(length):
    if length < 128:
        return bytes([length])
    raw = length.to_bytes((length.bit_length() + 7) // 8, "big")
    return bytes([0x80 | len(raw)]) + raw

def der(tag, body):
    return bytes([tag]) + der_len(len(body)) + body

def der_int(raw):
    raw = raw.lstrip(b"\x00") or b"\x00"
    if raw[0] & 0x80:
        raw = b"\x00" + raw
    return der(0x02, raw)

def rsa_jwk_to_pem(jwk):
    n = b64url_decode(jwk["n"])
    e = b64url_decode(jwk["e"])
    rsa_pub = der(0x30, der_int(n) + der_int(e))
    alg_id = der(0x30, der(0x06, b"\x2a\x86\x48\x86\xf7\x0d\x01\x01\x01") + der(0x05, b""))
    spki = der(0x30, alg_id + der(0x03, b"\x00" + rsa_pub))
    body = base64.encodebytes(spki).replace(b"\n", b"")
    lines = [b"-----BEGIN PUBLIC KEY-----"]
    lines += [body[i:i+64] for i in range(0, len(body), 64)]
    lines.append(b"-----END PUBLIC KEY-----")
    return b"\n".join(lines) + b"\n"

def verify_rs256(header, token, payload, binding):
    if header.get("alg") != "RS256":
        raise ValueError("MAA token is not RS256")
    jku = header.get("jku") or ""
    kid = header.get("kid") or ""
    if not jku.startswith("https://") or ".attest.azure.net/" not in jku:
        raise ValueError("MAA token jku is not an Azure Attestation JWKS URL")
    if not kid:
        raise ValueError("MAA token missing kid")
    with urllib.request.urlopen(jku, timeout=20, context=ssl.create_default_context()) as response:
        jwks = json.loads(response.read().decode("utf-8"))
    jwk = next((key for key in jwks.get("keys", []) if key.get("kid") == kid), None)
    if not jwk:
        raise ValueError("MAA token kid not present in JWKS")
    signing_input, signature = token.rsplit(".", 1)
    with tempfile.TemporaryDirectory(prefix="mayhem-maa-verify.") as tmp:
        pub = os.path.join(tmp, "pub.pem")
        sig = os.path.join(tmp, "sig.bin")
        msg = os.path.join(tmp, "msg")
        open(pub, "wb").write(rsa_jwk_to_pem(jwk))
        open(sig, "wb").write(b64url_decode(signature))
        open(msg, "wb").write(signing_input.encode("ascii"))
        result = subprocess.run(
            ["openssl", "dgst", "-sha256", "-verify", pub, "-signature", sig, msg],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=20,
        )
        if result.returncode != 0:
            raise ValueError(f"MAA token signature failed: {result.stderr.strip() or result.stdout.strip()}")
    now = int(time.time())
    if int(payload.get("nbf", 0)) > now + 300 or int(payload.get("exp", 0)) < now - 300:
        raise ValueError("MAA token is not currently valid")
    issuer = payload.get("iss") or ""
    if not issuer.startswith("https://") or ".attest.azure.net" not in issuer:
        raise ValueError("MAA issuer is not Azure Attestation")
    if payload.get("x-ms-attestation-type") != "sevsnpvm":
        raise ValueError("MAA token is not a SEV-SNP VM attestation")
    if payload.get("x-ms-compliance-status") != "azure-compliant-cvm":
        raise ValueError("MAA compliance status is not azure-compliant-cvm")
    expected_user_data = hashlib.sha512(
        json.dumps({"user-claims": {"nonce": binding}}).encode("utf-8")
    ).hexdigest().upper()
    runtime = payload.get("x-ms-runtime") or {}
    actual_user_data = str(runtime.get("user-data") or "").upper()
    if actual_user_data != expected_user_data:
        raise ValueError("MAA user-data digest does not bind the Mayhem quote nonce")
    if payload.get("x-ms-sevsnpvm-is-debuggable") is not False:
        raise ValueError("SEV-SNP VM is debuggable")
    if payload.get("x-ms-sevsnpvm-migration-allowed") is not False:
        raise ValueError("SEV-SNP VM migration is allowed")

def first_jwt_with_alg(text, alg):
    for token in re.findall(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+", text):
        try:
            header = json.loads(b64url_decode(token.split(".", 1)[0]).decode("utf-8"))
            payload = json.loads(b64url_decode(token.split(".")[1]).decode("utf-8"))
        except Exception:
            continue
        if header.get("alg") == alg:
            return token, header, payload
    raise ValueError(f"no {alg} JWT found")

def parse_pcrs(text):
    pcrs = {}
    bank = None
    for line in text.splitlines():
        bank_match = re.match(r"\s*(sha\d+):\s*$", line)
        if bank_match:
            bank = bank_match.group(1).lower()
            continue
        value_match = re.match(r"\s*(\d+)\s*:\s*0x([0-9A-Fa-f]+)\s*$", line)
        if bank and value_match:
            pcrs[f"{bank}:{value_match.group(1)}"] = value_match.group(2).lower()
    return pcrs

def matched_workload(golden_layers, pcr_text):
    pcrs = parse_pcrs(pcr_text)
    wanted = (golden_layers or {}).get("workload") or {}
    matched = {}
    for name, values in wanted.items():
        values = {str(v).lower().removeprefix("0x") for v in (values if isinstance(values, list) else list(values or []))}
        candidates = []
        m = re.search(r"(?:pcr[_-]?)(\d+)", name)
        if m:
            candidates.append(pcrs.get(f"sha256:{m.group(1)}"))
        candidates.extend(pcrs.values())
        for value in candidates:
            if value and value in values:
                matched[name] = value
                break
    return matched

def load_request():
    with open(os.environ["MAYHEM_AZURE_NCC_VERIFY_INPUT_FILE"], "r", encoding="utf-8") as fh:
        return json.loads(fh.read())

def maybe_live_evidence(kind, binding):
    target = os.environ.get("MAYHEM_AZURE_NCC_VERIFY_SSH_TARGET", "").strip()
    remote_command = os.environ.get("MAYHEM_AZURE_NCC_VERIFY_REMOTE_QUOTE_COMMAND", "").strip()
    if not target and not remote_command:
        return None
    if not target or not remote_command:
        raise ValueError("set both MAYHEM_AZURE_NCC_VERIFY_SSH_TARGET and MAYHEM_AZURE_NCC_VERIFY_REMOTE_QUOTE_COMMAND")
    known_hosts = os.environ.get("MAYHEM_AZURE_NCC_VERIFY_KNOWN_HOSTS", "").strip()
    key = os.environ.get("MAYHEM_AZURE_NCC_VERIFY_SSH_KEY", "").strip()
    opts = [
        "-F", "/dev/null",
        "-o", "BatchMode=yes",
        "-o", "PreferredAuthentications=publickey",
        "-o", "PasswordAuthentication=no",
        "-o", "KbdInteractiveAuthentication=no",
        "-o", "ConnectTimeout=8",
        "-o", "StrictHostKeyChecking=no",
    ]
    if known_hosts:
        opts.extend(["-o", f"UserKnownHostsFile={known_hosts}"])
    if key:
        opts.extend(["-i", key, "-o", "IdentitiesOnly=yes"])
    remote = (
        f"MAYHEM_HW_QUOTE_BINDING={shlex.quote(binding)} "
        f"MAYHEM_HW_QUOTE_KIND={shlex.quote(kind)} "
        f"{shlex.quote(remote_command)}"
    )
    result = subprocess.run(
        ["ssh", *opts, target, remote],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=int(os.environ.get("MAYHEM_AZURE_NCC_VERIFY_SSH_TIMEOUT_SECONDS", "180")),
    )
    if result.returncode != 0:
        raise ValueError(f"live Azure verifier quote command failed: {result.stderr.strip() or result.stdout.strip()}")
    quote = json.loads(result.stdout)
    return json.loads(quote["evidence"])

try:
    request = load_request()
    kind = request.get("kind") or os.environ.get("MAYHEM_HW_VERIFY_KIND") or "nvidia_nvtrust_offline_jwt"
    if kind != "nvidia_nvtrust_offline_jwt":
        fail(f"unsupported quote kind {kind}")
    binding = (request.get("expected_binding") or os.environ.get("MAYHEM_HW_VERIFY_BINDING") or "").lower()
    if not re.fullmatch(r"[0-9a-f]{64}", binding):
        fail("expected binding is not a 32-byte hex digest")
    quote = request.get("quote") or {}
    evidence = json.loads(quote.get("evidence") or "{}")
    live = maybe_live_evidence(kind, binding)
    if live is not None:
        evidence = live
    if evidence.get("binding") != binding:
        fail("evidence binding does not match gateway binding")
    platform_id = evidence.get("platform_id") or (quote.get("metadata") or {}).get("platform_id") or request.get("declared_platform")
    if platform_id != "azure-ncc":
        fail(f"unsupported Tier-3 platform {platform_id!r}")
    gpu = evidence.get("gpu") or {}
    gpu_log = gpu.get("log") or ""
    required_gpu = [
        "GPU Attestation is Successful",
        "nonce in the SPDM GET MEASUREMENT request message is matching",
        "GPU attestation report certificate chain validation successful",
        "Attestation report signature verification successful",
        "driver RIM signature verification successful",
        "vbios RIM signature verification successful",
        "runtime measurements are matching with the golden measurements",
    ]
    for needle in required_gpu:
        if needle.lower() not in gpu_log.lower():
            fail(f"NVIDIA GPU evidence missing: {needle}")
    if binding not in gpu_log.lower():
        fail("NVIDIA GPU evidence is not nonce-bound to the Mayhem binding")
    cc_state = evidence.get("cc_state") or ""
    if "CC status: ON" not in cc_state or "CC Environment: PRODUCTION" not in cc_state or "ready" not in cc_state.lower():
        fail("NVIDIA CC mode is not ON/PRODUCTION/ready")
    cpu = evidence.get("cpu") or {}
    cpu_log = cpu.get("log") or ""
    if "Attested Platform Successfully" not in cpu_log:
        fail("CPU/CVM evidence did not attest successfully")
    maa_token, maa_header, maa_payload = first_jwt_with_alg(cpu_log, "RS256")
    verify_rs256(maa_header, maa_token, maa_payload, binding)
    matched = matched_workload(request.get("golden_measurement_layers") or {}, evidence.get("pcrs") or "")
    if not matched:
        fail("no workload PCR matched the admin golden set")
    print(json.dumps({
        "ok": True,
        "kind": kind,
        "binding": binding,
        "att_tier": 3,
        "roots": [
            "nvidia_gpu_cert_chain",
            "nvidia_driver_rim",
            "nvidia_vbios_rim",
            "azure_maa_jwt_jwks_issuer_nonce_claims",
        ],
        "matched_measurements": {"workload": matched},
        "platform_id": jstr(platform_id),
        "region": jstr(evidence.get("region")),
        "snp_chip_family": jstr(maa_payload.get("x-ms-sevsnpvm-chip-family") or cpu.get("snp_chip_family")),
        "snp_chip_id": jstr(maa_payload.get("x-ms-sevsnpvm-chipid")),
        "snp_tcb": jstr(cpu.get("snp_tcb")),
        "snp_firmware_svn": jstr(maa_payload.get("x-ms-sevsnpvm-snpfw-svn") or cpu.get("snp_firmware_svn")),
        "gpu_model": jstr(gpu.get("model")),
        "gpu_driver": jstr(gpu.get("driver")),
        "gpu_vbios": jstr(gpu.get("vbios")),
        "alert": {
            "platform_id": jstr(platform_id),
            "region": jstr(evidence.get("region")),
            "gpu_model": jstr(gpu.get("model")),
            "gpu_driver": jstr(gpu.get("driver")),
            "gpu_vbios": jstr(gpu.get("vbios")),
        },
    }, sort_keys=True))
except Exception as exc:
    fail(str(exc))
PY
