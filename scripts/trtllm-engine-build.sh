#!/usr/bin/env bash
set -euo pipefail

IMAGE="nvcr.io/nvidia/tensorrt-llm/release:1.2.0"
MODEL_DIR=""
CHECKPOINT_DIR=""
ENGINE_DIR=""
CALIB_DATASET=""
QFORMAT="nvfp4"
KV_CACHE_DTYPE=""
CTX_SIZE="1024"
MAX_BATCH_SIZE="1"
MAX_NUM_TOKENS=""
OPT_NUM_TOKENS=""
CALIB_SIZE="8"
CALIB_MAX_SEQ_LENGTH="128"
TP_SIZE="1"
WORKERS="1"
MOUNT_ROOT=""
GPT_ATTENTION_PLUGIN=""
CONTEXT_FMHA=""
REMOVE_INPUT_PADDING=""
USE_PAGED_CONTEXT_FMHA=""
USE_FP8_CONTEXT_FMHA=""
FUSE_FP4_QUANT=""
MULTIPLE_PROFILES=""
PROFILING_VERBOSITY=""
INPUT_TIMING_CACHE=""
OUTPUT_TIMING_CACHE=""
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
  --opt-num-tokens N        Preferred batched token profile for TensorRT tactic choice
  --calib-dataset PATH      Local HF dataset dir with train split/text column
  --calib-size N            Calibration samples for quantized export (default: 8)
  --calib-max-seq-length N  Calibration sequence length (default: 128)
  --tp-size N               Tensor parallel degree (default: 1)
  --workers N               trtllm-build workers (default: 1)
  --gpt-attention-plugin V  auto,float16,bfloat16,float32,int32,disable
  --context-fmha V          enable or disable
  --remove-input-padding V  enable or disable
  --use-paged-context-fmha V
                            enable or disable
  --use-fp8-context-fmha V  enable or disable
  --fuse-fp4-quant V        enable or disable
  --multiple-profiles V     enable or disable
  --profiling-verbosity V   layer_names_only,detailed,none
  --input-timing-cache PATH Read TensorRT timing cache when present
  --output-timing-cache PATH
                            Write TensorRT timing cache
  --image IMAGE             TensorRT-LLM container image
                            (default: nvcr.io/nvidia/tensorrt-llm/release:1.2.0)
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
    --opt-num-tokens) OPT_NUM_TOKENS="$2"; shift 2 ;;
    --calib-size) CALIB_SIZE="$2"; shift 2 ;;
    --calib-max-seq-length) CALIB_MAX_SEQ_LENGTH="$2"; shift 2 ;;
    --tp-size) TP_SIZE="$2"; shift 2 ;;
    --workers) WORKERS="$2"; shift 2 ;;
    --gpt-attention-plugin) GPT_ATTENTION_PLUGIN="$2"; shift 2 ;;
    --context-fmha) CONTEXT_FMHA="$2"; shift 2 ;;
    --remove-input-padding) REMOVE_INPUT_PADDING="$2"; shift 2 ;;
    --use-paged-context-fmha) USE_PAGED_CONTEXT_FMHA="$2"; shift 2 ;;
    --use-fp8-context-fmha) USE_FP8_CONTEXT_FMHA="$2"; shift 2 ;;
    --fuse-fp4-quant) FUSE_FP4_QUANT="$2"; shift 2 ;;
    --multiple-profiles) MULTIPLE_PROFILES="$2"; shift 2 ;;
    --profiling-verbosity) PROFILING_VERBOSITY="$2"; shift 2 ;;
    --input-timing-cache) INPUT_TIMING_CACHE="$2"; shift 2 ;;
    --output-timing-cache) OUTPUT_TIMING_CACHE="$2"; shift 2 ;;
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
if [[ -n "$INPUT_TIMING_CACHE" ]]; then
  INPUT_TIMING_CACHE="$(realpath -m "$INPUT_TIMING_CACHE")"
fi
if [[ -n "$OUTPUT_TIMING_CACHE" ]]; then
  OUTPUT_TIMING_CACHE="$(realpath -m "$OUTPUT_TIMING_CACHE")"
fi
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

if [[ -n "$CALIB_DATASET" ]]; then
  CALIB_DATASET="$(realpath "$CALIB_DATASET")"
elif [[ "$QFORMAT" != "full_prec" ]]; then
  CALIB_DATASET="$(dirname "$CHECKPOINT_DIR")/.mayhem-trt-calib"
fi

has_engine_payload() {
  [[ -d "$1" ]] && find "$1" -maxdepth 1 -type f \( -name '*.engine' -o -name '*.plan' \) | grep -q .
}

has_trt_checkpoint() {
  [[ -f "$1/config.json" ]] &&
    grep -q '"architecture"' "$1/config.json" &&
    find "$1" -maxdepth 1 -type f \( -name 'rank*.safetensors' -o -name 'rank*.bin' \) | grep -q .
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
      { [[ -z "$INPUT_TIMING_CACHE" ]] || contains_path "$parent" "$INPUT_TIMING_CACHE"; } &&
      { [[ -z "$OUTPUT_TIMING_CACHE" ]] || contains_path "$parent" "$OUTPUT_TIMING_CACHE"; } &&
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
  -e OPT_NUM_TOKENS="$OPT_NUM_TOKENS" \
  -e CALIB_SIZE="$CALIB_SIZE" \
  -e CALIB_MAX_SEQ_LENGTH="$CALIB_MAX_SEQ_LENGTH" \
  -e TP_SIZE="$TP_SIZE" \
  -e WORKERS="$WORKERS" \
  -e GPT_ATTENTION_PLUGIN="$GPT_ATTENTION_PLUGIN" \
  -e CONTEXT_FMHA="$CONTEXT_FMHA" \
  -e REMOVE_INPUT_PADDING="$REMOVE_INPUT_PADDING" \
  -e USE_PAGED_CONTEXT_FMHA="$USE_PAGED_CONTEXT_FMHA" \
  -e USE_FP8_CONTEXT_FMHA="$USE_FP8_CONTEXT_FMHA" \
  -e FUSE_FP4_QUANT="$FUSE_FP4_QUANT" \
  -e MULTIPLE_PROFILES="$MULTIPLE_PROFILES" \
  -e PROFILING_VERBOSITY="$PROFILING_VERBOSITY" \
  -e INPUT_TIMING_CACHE="$INPUT_TIMING_CACHE" \
  -e OUTPUT_TIMING_CACHE="$OUTPUT_TIMING_CACHE" \
  -e HOST_UID="$HOST_UID" \
  -e HOST_GID="$HOST_GID" \
  --entrypoint bash "$IMAGE" -s <<'INNER'
set -euo pipefail

has_engine_payload() {
  [[ -d "$1" ]] && find "$1" -maxdepth 1 -type f \( -name '*.engine' -o -name '*.plan' \) | grep -q .
}

has_trt_checkpoint() {
  [[ -f "$1/config.json" ]] &&
    grep -q '"architecture"' "$1/config.json" &&
    find "$1" -maxdepth 1 -type f \( -name 'rank*.safetensors' -o -name 'rank*.bin' \) | grep -q .
}

reject_modelopt_hf_artifact() {
  python3 - <<'PY'
import json
import os
import sys

model_dir = os.environ["MODEL_DIR"]
quant_paths = [
    os.path.join(model_dir, "hf_quant_config.json"),
    os.path.join(model_dir, "quant_config.json"),
]
config_path = os.path.join(model_dir, "config.json")
quant = {}
for path in quant_paths:
    if os.path.exists(path):
        with open(path) as f:
            quant.update(json.load(f).get("quantization", {}))
if os.path.exists(config_path):
    with open(config_path) as f:
        config = json.load(f)
    quant.update(config.get("quantization_config", {}))

producer = quant.get("producer") if isinstance(quant, dict) else None
producer_name = ""
if isinstance(producer, dict):
    producer_name = str(producer.get("name") or "")
algo = str(quant.get("quant_algo") or quant.get("algorithm") or "").upper() if isinstance(quant, dict) else ""
is_modelopt = producer_name.lower() == "modelopt" or bool(algo)
if is_modelopt:
    sys.stderr.write(
        f"{model_dir} is a pre-quantized ModelOpt HF artifact"
        f"{f' ({algo})' if algo else ''}, not a TensorRT-LLM checkpoint directory. "
        "Do not patch config.json to add TensorRT metadata: trtllm-build still expects "
        "rank-format checkpoint weights, not HF safetensors. Provide the BF16/FP16 "
        "admin source artifact so this helper can export the TensorRT-LLM checkpoint, "
        "or provide an admin-exported TensorRT-LLM checkpoint/prebuilt engine bundle.\n"
    )
    sys.exit(64)
PY
}

copy_tokenizer_metadata() {
  local name
  for name in \
    tokenizer.json \
    tokenizer_config.json \
    special_tokens_map.json \
    generation_config.json \
    vocab.json \
    merges.txt \
    tokenizer.model \
    sentencepiece.bpe.model \
    added_tokens.json; do
    if [[ -f "$MODEL_DIR/$name" && ! -f "$CHECKPOINT_DIR/$name" ]]; then
      cp -a "$MODEL_DIR/$name" "$CHECKPOINT_DIR/$name"
    fi
  done
}

if ! has_trt_checkpoint "$CHECKPOINT_DIR"; then
  tmp="${CHECKPOINT_DIR}.tmp.$$"
  rm -rf "$tmp"
  mkdir -p "$tmp"
  trap 'rm -rf "$tmp"' EXIT
  if has_trt_checkpoint "$MODEL_DIR"; then
    cp -a "$MODEL_DIR"/. "$tmp"/
  else
    reject_modelopt_hf_artifact
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

copy_tokenizer_metadata

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
  append_build_arg() {
    local flag="$1"
    local value="$2"
    if [[ -n "$value" ]]; then
      build_args+=("$flag" "$value")
    fi
  }
  append_build_arg --opt_num_tokens "$OPT_NUM_TOKENS"
  append_build_arg --gpt_attention_plugin "$GPT_ATTENTION_PLUGIN"
  append_build_arg --context_fmha "$CONTEXT_FMHA"
  append_build_arg --remove_input_padding "$REMOVE_INPUT_PADDING"
  append_build_arg --use_paged_context_fmha "$USE_PAGED_CONTEXT_FMHA"
  append_build_arg --use_fp8_context_fmha "$USE_FP8_CONTEXT_FMHA"
  append_build_arg --fuse_fp4_quant "$FUSE_FP4_QUANT"
  append_build_arg --multiple_profiles "$MULTIPLE_PROFILES"
  append_build_arg --profiling_verbosity "$PROFILING_VERBOSITY"
  append_build_arg --input_timing_cache "$INPUT_TIMING_CACHE"
  append_build_arg --output_timing_cache "$OUTPUT_TIMING_CACHE"
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
    "opt_num_tokens": int(os.environ["OPT_NUM_TOKENS"]) if os.environ["OPT_NUM_TOKENS"] else None,
    "tp_size": int(os.environ["TP_SIZE"]),
    "build_options": {
        "gpt_attention_plugin": os.environ["GPT_ATTENTION_PLUGIN"] or None,
        "context_fmha": os.environ["CONTEXT_FMHA"] or None,
        "remove_input_padding": os.environ["REMOVE_INPUT_PADDING"] or None,
        "use_paged_context_fmha": os.environ["USE_PAGED_CONTEXT_FMHA"] or None,
        "use_fp8_context_fmha": os.environ["USE_FP8_CONTEXT_FMHA"] or None,
        "fuse_fp4_quant": os.environ["FUSE_FP4_QUANT"] or None,
        "multiple_profiles": os.environ["MULTIPLE_PROFILES"] or None,
        "profiling_verbosity": os.environ["PROFILING_VERBOSITY"] or None,
        "input_timing_cache": os.environ["INPUT_TIMING_CACHE"] or None,
        "output_timing_cache": os.environ["OUTPUT_TIMING_CACHE"] or None,
    },
    "files": files,
}
(engine_dir / "mayhem-trtllm-engine-manifest.json").write_text(
    json.dumps(manifest, indent=2, sort_keys=True) + "\n"
)
print(json.dumps(manifest, indent=2, sort_keys=True))
PY

if [[ -n "${HOST_UID:-}" && -n "${HOST_GID:-}" ]]; then
  chown -R "$HOST_UID:$HOST_GID" "$CHECKPOINT_DIR" "$ENGINE_DIR"
fi
INNER

if ! has_engine_payload "$ENGINE_DIR"; then
  echo "TensorRT-LLM engine payload missing from $ENGINE_DIR" >&2
  exit 1
fi

echo "TensorRT-LLM engine ready: $ENGINE_DIR"
