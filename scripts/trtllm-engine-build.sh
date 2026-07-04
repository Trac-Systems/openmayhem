#!/usr/bin/env bash
set -euo pipefail

IMAGE="nvcr.io/nvidia/tensorrt-llm/release:1.3.0rc20"
MODEL_DIR=""
CHECKPOINT_DIR=""
ENGINE_DIR=""
CALIB_DATASET=""
QFORMAT="nvfp4"
KV_CACHE_DTYPE=""
CTX_SIZE="1024"
MAX_BATCH_SIZE="1"
MAX_NUM_TOKENS=""
CALIB_SIZE="8"
CALIB_MAX_SEQ_LENGTH="128"
TP_SIZE="1"
WORKERS="1"
MOUNT_ROOT=""
FORCE=0

usage() {
  cat <<'USAGE'
Usage: scripts/trtllm-engine-build.sh --model-dir PATH --engine-dir PATH [options]

Build a TensorRT-LLM engine directory from an admin-approved model artifact.
The output engine directory is what mayhem provider start requires for trt-llm
serving; providers still cannot choose models, prices, or canonical rooms.

Required:
  --model-dir PATH          Hugging Face/ModelOpt or TensorRT-LLM checkpoint dir
  --engine-dir PATH         Output engine directory containing .engine/.plan files

Options:
  --checkpoint-dir PATH     Intermediate TensorRT-LLM checkpoint dir
  --qformat NAME            nvfp4, fp8, full_prec, ... (default: nvfp4)
  --kv-cache-dtype NAME     fp8, int8, or omit for model/default KV cache
  --ctx-size N              Max input/sequence length for the engine (default: 1024)
  --max-batch-size N        Engine max batch size (default: 1)
  --max-num-tokens N        Engine max batched tokens (default: ctx-size)
  --calib-dataset PATH      Local HF dataset dir with train split/text column
  --calib-size N            Calibration samples for quantized export (default: 8)
  --calib-max-seq-length N  Calibration sequence length (default: 128)
  --tp-size N               Tensor parallel degree (default: 1)
  --workers N               trtllm-build workers (default: 1)
  --image IMAGE             TensorRT-LLM container image
  --mount-root PATH         Host path mounted into the container at the same path
  --force                   Rebuild checkpoint and engine outputs
  -h, --help                Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model-dir) MODEL_DIR="$2"; shift 2 ;;
    --checkpoint-dir) CHECKPOINT_DIR="$2"; shift 2 ;;
    --engine-dir) ENGINE_DIR="$2"; shift 2 ;;
    --calib-dataset) CALIB_DATASET="$2"; shift 2 ;;
    --qformat) QFORMAT="$2"; shift 2 ;;
    --kv-cache-dtype) KV_CACHE_DTYPE="$2"; shift 2 ;;
    --ctx-size) CTX_SIZE="$2"; shift 2 ;;
    --max-batch-size) MAX_BATCH_SIZE="$2"; shift 2 ;;
    --max-num-tokens) MAX_NUM_TOKENS="$2"; shift 2 ;;
    --calib-size) CALIB_SIZE="$2"; shift 2 ;;
    --calib-max-seq-length) CALIB_MAX_SEQ_LENGTH="$2"; shift 2 ;;
    --tp-size) TP_SIZE="$2"; shift 2 ;;
    --workers) WORKERS="$2"; shift 2 ;;
    --image) IMAGE="$2"; shift 2 ;;
    --mount-root) MOUNT_ROOT="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$MODEL_DIR" || -z "$ENGINE_DIR" ]]; then
  usage >&2
  exit 2
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for the TensorRT-LLM engine build helper" >&2
  exit 1
fi

MODEL_DIR="$(realpath "$MODEL_DIR")"
ENGINE_DIR="$(realpath -m "$ENGINE_DIR")"
if [[ -z "$CHECKPOINT_DIR" ]]; then
  CHECKPOINT_DIR="$(dirname "$ENGINE_DIR")/trtllm-checkpoint-$(basename "$ENGINE_DIR")"
fi
CHECKPOINT_DIR="$(realpath -m "$CHECKPOINT_DIR")"
MAX_NUM_TOKENS="${MAX_NUM_TOKENS:-$CTX_SIZE}"

if [[ -n "$CALIB_DATASET" ]]; then
  CALIB_DATASET="$(realpath "$CALIB_DATASET")"
elif [[ "$QFORMAT" != "full_prec" ]]; then
  CALIB_DATASET="$(dirname "$CHECKPOINT_DIR")/.mayhem-trt-calib"
fi

has_engine_payload() {
  [[ -d "$1" ]] && find "$1" -maxdepth 1 -type f \( -name '*.engine' -o -name '*.plan' \) | grep -q .
}

has_trt_checkpoint() {
  [[ -f "$1/config.json" ]] && grep -q '"architecture"' "$1/config.json"
}

contains_path() {
  local parent="$1"
  local child="$2"
  [[ "$child" == "$parent" || "$child" == "$parent"/* ]]
}

common_mount_root() {
  local parent
  parent="$(dirname "$MODEL_DIR")"
  while [[ "$parent" != "/" ]]; do
    if contains_path "$parent" "$MODEL_DIR" &&
      contains_path "$parent" "$CHECKPOINT_DIR" &&
      contains_path "$parent" "$ENGINE_DIR" &&
      { [[ -z "$CALIB_DATASET" ]] || contains_path "$parent" "$CALIB_DATASET"; }; then
      printf '%s\n' "$parent"
      return
    fi
    parent="$(dirname "$parent")"
  done
  printf '/\n'
}

if [[ -z "$MOUNT_ROOT" ]]; then
  MOUNT_ROOT="$(common_mount_root)"
else
  MOUNT_ROOT="$(realpath "$MOUNT_ROOT")"
fi

if [[ "$FORCE" -eq 1 ]]; then
  rm -rf "$CHECKPOINT_DIR" "$ENGINE_DIR"
fi

mkdir -p "$(dirname "$CHECKPOINT_DIR")" "$(dirname "$ENGINE_DIR")"
if [[ -n "$CALIB_DATASET" && "$QFORMAT" != "full_prec" && ! -d "$CALIB_DATASET" ]]; then
  mkdir -p "$CALIB_DATASET"
  cat >"$CALIB_DATASET/train.jsonl" <<'JSONL'
{"text":"Mayhem calibration prompt about audited inference receipts and deterministic provider settlement."}
{"text":"The admin approved enclave serves the exact signed model artifact and records evidence roots."}
{"text":"Short calibration text for a local TensorRT-LLM export without fetching external datasets."}
{"text":"Providers opt in to canonical rooms; they do not define prices, models, rooms, or payout terms."}
JSONL
fi

docker run --rm -i --gpus all --ipc=host \
  --ulimit memlock=-1 --ulimit stack=67108864 \
  -v "$MOUNT_ROOT:$MOUNT_ROOT" \
  -e MODEL_DIR="$MODEL_DIR" \
  -e CHECKPOINT_DIR="$CHECKPOINT_DIR" \
  -e ENGINE_DIR="$ENGINE_DIR" \
  -e CALIB_DATASET="$CALIB_DATASET" \
  -e QFORMAT="$QFORMAT" \
  -e KV_CACHE_DTYPE="$KV_CACHE_DTYPE" \
  -e CTX_SIZE="$CTX_SIZE" \
  -e MAX_BATCH_SIZE="$MAX_BATCH_SIZE" \
  -e MAX_NUM_TOKENS="$MAX_NUM_TOKENS" \
  -e CALIB_SIZE="$CALIB_SIZE" \
  -e CALIB_MAX_SEQ_LENGTH="$CALIB_MAX_SEQ_LENGTH" \
  -e TP_SIZE="$TP_SIZE" \
  -e WORKERS="$WORKERS" \
  --entrypoint bash "$IMAGE" -s <<'INNER'
set -euo pipefail

has_engine_payload() {
  [[ -d "$1" ]] && find "$1" -maxdepth 1 -type f \( -name '*.engine' -o -name '*.plan' \) | grep -q .
}

has_trt_checkpoint() {
  [[ -f "$1/config.json" ]] && grep -q '"architecture"' "$1/config.json"
}

if ! has_trt_checkpoint "$CHECKPOINT_DIR"; then
  tmp="${CHECKPOINT_DIR}.tmp.$$"
  rm -rf "$tmp"
  mkdir -p "$tmp"
  trap 'rm -rf "$tmp"' EXIT
  if has_trt_checkpoint "$MODEL_DIR"; then
    cp -a "$MODEL_DIR"/. "$tmp"/
  else
    if [[ "$QFORMAT" != "full_prec" && -f "$MODEL_DIR/hf_quant_config.json" ]]; then
      python3 - <<'PY'
import json
import os
import sys
path = os.path.join(os.environ["MODEL_DIR"], "hf_quant_config.json")
with open(path) as f:
    quant = json.load(f).get("quantization", {})
algo = str(quant.get("quant_algo") or "").upper()
if algo:
    sys.stderr.write(
        f"{path} declares a pre-quantized ModelOpt {algo} HF checkpoint. "
        "The legacy TensorRT-LLM engine builder needs a TensorRT-LLM checkpoint "
        "with config.json architecture metadata; provide that admin-exported "
        "checkpoint as --model-dir/--checkpoint-dir instead of re-quantizing this HF artifact.\n"
    )
    sys.exit(64)
PY
    fi
    args=(
      python3 /app/tensorrt_llm/examples/quantization/quantize.py
      --model_dir "$MODEL_DIR"
      --qformat "$QFORMAT"
      --output_dir "$tmp"
      --tp_size "$TP_SIZE"
      --batch_size 1
      --calib_size "$CALIB_SIZE"
      --calib_max_seq_length "$CALIB_MAX_SEQ_LENGTH"
    )
    if [[ "$QFORMAT" != "full_prec" ]]; then
      args+=(--calib_dataset "$CALIB_DATASET")
    fi
    if [[ -n "$KV_CACHE_DTYPE" ]]; then
      args+=(--kv_cache_dtype "$KV_CACHE_DTYPE")
    fi
    "${args[@]}"
  fi
  rm -rf "$CHECKPOINT_DIR"
  mv "$tmp" "$CHECKPOINT_DIR"
  trap - EXIT
fi

if ! has_engine_payload "$ENGINE_DIR"; then
  tmp="${ENGINE_DIR}.tmp.$$"
  rm -rf "$tmp"
  trap 'rm -rf "$tmp"' EXIT
  build_args=(
    trtllm-build
    --checkpoint_dir "$CHECKPOINT_DIR"
    --output_dir "$tmp"
    --max_batch_size "$MAX_BATCH_SIZE"
    --max_input_len "$CTX_SIZE"
    --max_seq_len "$CTX_SIZE"
    --max_num_tokens "$MAX_NUM_TOKENS"
    --workers "$WORKERS"
    --log_level info
  )
  case "$QFORMAT" in
    nvfp4) build_args+=(--gemm_plugin nvfp4) ;;
    fp8) build_args+=(--gemm_plugin fp8) ;;
  esac
  "${build_args[@]}"
  rm -rf "$ENGINE_DIR"
  mv "$tmp" "$ENGINE_DIR"
  trap - EXIT
fi

if ! has_engine_payload "$ENGINE_DIR"; then
  echo "TensorRT-LLM build finished without .engine or .plan payload in $ENGINE_DIR" >&2
  exit 1
fi

python3 - <<'PY'
import hashlib
import json
import os
from pathlib import Path

engine_dir = Path(os.environ["ENGINE_DIR"])
files = []
for path in sorted(engine_dir.iterdir()):
    if not path.is_file():
        continue
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    files.append({"name": path.name, "bytes": path.stat().st_size, "sha256": h.hexdigest()})

manifest = {
    "schema_version": 1,
    "kind": "mayhem.trtllm.engine_build",
    "model_dir": os.environ["MODEL_DIR"],
    "checkpoint_dir": os.environ["CHECKPOINT_DIR"],
    "engine_dir": os.environ["ENGINE_DIR"],
    "qformat": os.environ["QFORMAT"],
    "kv_cache_dtype": os.environ["KV_CACHE_DTYPE"] or None,
    "ctx_size": int(os.environ["CTX_SIZE"]),
    "max_batch_size": int(os.environ["MAX_BATCH_SIZE"]),
    "max_num_tokens": int(os.environ["MAX_NUM_TOKENS"]),
    "tp_size": int(os.environ["TP_SIZE"]),
    "files": files,
}
(engine_dir / "mayhem-trtllm-engine-manifest.json").write_text(
    json.dumps(manifest, indent=2, sort_keys=True) + "\n"
)
print(json.dumps(manifest, indent=2, sort_keys=True))
PY
INNER

if ! has_engine_payload "$ENGINE_DIR"; then
  echo "TensorRT-LLM engine payload missing from $ENGINE_DIR" >&2
  exit 1
fi

echo "TensorRT-LLM engine ready: $ENGINE_DIR"
