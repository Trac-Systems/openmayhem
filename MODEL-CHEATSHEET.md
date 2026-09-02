# Canonical Model Cheatsheet

This is the provider and operator reference for every model currently present in
the signed canonical catalog. It is derived from
[`catalog/models.json`](catalog/models.json), its
[detached signature](catalog/signatures/models.json.sig), the canaries named
by that catalog, the managed-runtime locks in
[`python_runtime.rs`](crates/mayhem-cli/src/python_runtime.rs), and
[`CALIBRATION.md`](CALIBRATION.md).

The live ledger remains authoritative for active enclave IDs, rooms, prices,
routes, and revisions. Never copy an enclave ID or price from documentation:

```bash
mayhem models --gateway
```

## v0.2.117 runtime status

The `0.2.117` source release keeps the previous source-install, managed
runtime, co-resident memory, endpoint-floor, Sulphur, Chatterbox, ACE-Step,
Needle, and Comfy workflow market fixes, and adds the current Comfy parts
inventory plus bounded workflow `input_files` for image/audio/video source
media. The dedicated Comfy provider/user reference is
[`COMFY-CHEATSHEET.md`](COMFY-CHEATSHEET.md); it lists every outcome class, the
current signed parts index, required part sets, and `/v1/workflows` usage.
Ledger-only workflow outcome classes expose endpoint family
`mayhem_comfy_workflows`, providers can resolve workflow enclaves without a
static `catalog/models.json` row, and workflow serving uses
`--artifact <comfy-runtime-dir>` for the local ComfyUI runtime checkout while
keeping the ledger artifact bound to the workflow class definition. Workflow
graph hashes canonicalize integer-valued JSON floats, so JS/Pear transport
normalization of `1.0` to `1` does not break signed workflow vouchers. Signed
Comfy upscaler parts may declare a scale; Krea+4x workflows must use v0.2.115 or
newer on both buyer gateways and providers so route requirements, vouchers, and
receipts meter the upscaled output dimensions.
Workflows that consume source media, such as lipsync/talking-video policies,
require v0.2.116 or newer on both buyer gateways and providers so the signed
request, route load, engine payload, and receipt agree on the input
image/audio/video media. Workflows that produce one MP4 containing both video
and audio require v0.2.117 or newer for provider startup canaries; the provider
accepts the muxed audio modality only after the A/V canary decoder extracts a
non-silent audio track.
Dedicated-VRAM reservations remain enforced. Needle still has
exactly two markets: `needle-cpu` across Linux, Windows x86_64, and Apple
Silicon macOS, and CUDA-only `needle-gpu` on supported Linux aarch64/x86_64 and
Windows x86_64 hosts. Apple Metal/MPS is not eligible for the GPU market.

## How to read this document

- **Hard requirement** means the signed catalog or release code enforces it.
  Failing it prevents admission or startup.
- **Measured guidance** reports a calibration observation. It helps size a
  machine but is not a new admission rule, throughput promise, or exact minimum.
- Artifact sizes are exact catalog bytes. GiB values are rounded only for
  readability.
- Catalog `tier: "launch"` is a publication stage, not a provider trust-tier
  floor. Every model below may be served at Tier 1. A provider that proves a
  higher tier is admitted at that tier; if a higher proof is unavailable or
  fails, admission falls through to the next lower provable tier. Tier 4 is an
  optional admin elevation, not a prerequisite for joining.
- Providers join permissionlessly. They do not create canonical models,
  enclaves, rooms, prices, or price brackets, and no admin manually approves a
  provider or its payout binding.

## Common provider workflow

Use the exact, case-sensitive model ID printed in this document:

```bash
mayhem models --gateway
mayhem doctor --provider-backend <backend>
mayhem up --provider --provider-enclave <exact-model-id> --yes
mayhem provider health --json
mayhem models --gateway
```

`mayhem up` resolves the active admin-published enclave, downloads the immutable
primary artifact and all signed sidecars, verifies their hashes and bindings,
seals them, runs the signed canary surface, joins admin-created canonical rooms,
and starts heartbeats. `HF_TOKEN` or `--hf-token-file` may authenticate and speed
up that download; it cannot select different weights. Do not clone upstream
weights, substitute projectors/encoders, or build an ad hoc Python environment.

For a text-generation model, catalog `ctx_max` is a ceiling, not an exact
provider setting. An explicit lower `--ctx` is preserved through heartbeat,
routing, vouchers, receipts, and reporting. Current price brackets are:

| Catalog ceiling | Required canonical brackets |
|---|---|
| Up to 8,192 | `le8k` |
| 8,193 through 32,768 | `le8k`, `le32k` |
| 32,769 through 131,072 | `le8k`, `le32k`, `le128k` |
| 131,073 through 262,144 | `le8k`, `le32k`, `le128k`, `le256k` |
| Above 262,144 | The preceding four plus `gt256k` |

A lower context does not create a new enclave and does not need admin approval.
If a provider changes its committed context, it signs its own leave/rejoin.

## Public API error codes

Every model class uses the same public gateway error taxonomy. Direct failures
return `error.code`, `error.category`, `error.retryable`, and optional
`error.safe_detail`; async artifact/workflow jobs expose the same classification
as `error_info` in `/v1/jobs/<id>`.

Use the code, not prose guessing, when triaging:

| Code | Model/provider interpretation |
|---|---|
| `request_exceeds_provider_capacity` | Request exceeds the signed envelope for the chosen model or workflow: context, media size, duration, frames, steps, output count, or input bytes. |
| `required_modality_unavailable` | The catalog entry exists but no live route serves the requested modality set. |
| `provider_admission_no_capacity` | A route exists but no provider accepted before the wait deadline; the lane may be full, busy, or draining. |
| `payment_rail_not_supported_by_provider` | Buyer selected a rail no live provider for that model accepts. |
| `insufficient_balance` | Buyer has too little unreserved credit on the selected rail. |
| `payment_reservation_failed` | Spend reservation failed before work began; retry after route/accounting health recovers. |
| `provider_transport_closed` / `provider_response_timeout` | Provider disconnected, restarted, or exceeded the session deadline. |
| `provider_response_invalid` / `provider_model_output_invalid` | Provider output did not satisfy the endpoint contract, including strict JSON/tool-response contracts. |
| `provider_verification_failed` | Signed provider data, receipt, canary, or attestation failed verification; do not keep retrying the same route unchanged. |
| `client_receive_rate_exceeded` | The caller or proxy is not reading the stream fast enough; use async job mode for long artifact/workflow requests. |

## Comfy workflow provider workflow

ComfyUI workflow classes are not ordinary single-weight model entries. They are
admin-created workflow outcome classes exposed through endpoint family
`mayhem_comfy_workflows` and served by the same provider, room, heartbeat,
voucher, receipt, and settlement path as the models below.

Discover workflow classes and live capacity from a local gateway:

```bash
curl 'http://127.0.0.1:11435/v1/models?endpoint_family=mayhem_comfy_workflows'
curl 'http://127.0.0.1:11435/v1/models?endpoint_family=mayhem_comfy_workflows&live=true'
```

To become a Comfy workflow provider, install the current source release, verify
the blessed runtime checkout, pull only the signed parts the machine intends to
serve, admit the outcome class, then start the resolved workflow enclave:

```bash
mayhem doctor --provider-backend comfyui
mayhem provider parts pull \
  --layout-dir <parts-index-layout> \
  --part-id <part-id> \
  --require-payload
mayhem provider parts add \
  --layout-dir <parts-index-layout> \
  --part-id <part-id>
mayhem provider parts admit \
  --outcome-class <workflow-class> \
  --runtime-id comfyui-v0.30.1 \
  --part-id <part-id> \
  --usable-bytes <size> \
  --working-set-bytes <size> \
  --reference-graph <path.json> \
  --reference-runtime <comfy-runtime-dir> \
  --reference-output-dir <proof-dir> \
  --write
mayhem up --yes
mayhem provider serve add <workflow-enclave-id> --artifact <comfy-runtime-dir> --workflow-class-definition <definition.json> --json
mayhem provider health --json
```

If the admitted class uses staged load/unload, pass its approved `--load-plan`
to `mayhem provider parts admit`. Without a load plan, all required parts must
fit together. A workflow provider is not routable until the saved admission
envelope exists and heartbeats advertise the matching `workflow_classes`; an
empty live query means no admitted provider is online, not that the catalog
lacks workflow enclaves. The `--artifact` path for workflow serving is the
local ComfyUI runtime checkout, not the ledger-pinned workflow class artifact.
If the signed catalog does not embed `workflow.outcome_class_definition`, add
`--workflow-class-definition <definition.json>` when starting the provider.

Current proven Krea 2 Turbo workflow lane:

| Field | Value |
|---|---|
| Outcome class / model | `image.heavy.le1_2mp` |
| Endpoint family | `mayhem_comfy_workflows` |
| Endpoint | `POST /v1/workflows` |
| Enclave ID | `1e2d23929e17ddb47b29fa75d9ad6c3c90cf447260d6e25d6d37e07033f85da3` |
| Canonical room | `3f5360b1f0f25f49a84f5acbff23e162` |
| Workflow class artifact root | `be5726d73958c4f3ec2fb89a17030308aad8059abfe884ae36ae9aad9c4724f1` |
| Workflow class source SHA-256 | `155ee5cf12374bdf24e4981614b5a21d6389c5b411390b6c852bd7f7e06d8f6b` |
| Runtime | `comfyui-v0.30.1` |
| Pricing unit | `megapixel_step` |
| 1024x1024, 8-step usage | `16` `megapixel_step` |
| Paid proof | `.42` provider through `.31` sponsored fiat gateway; session `36e05888bdeb8dce7a0fbb21a7bf4ad65efaa58e10043e1e057ab76b66524448`; artifact `openmayhem-krea-base-mercedes-salespitch-paid-v0.2.123.png`; BLAKE3 `a6ab8dec551c1d0e86256b50ee2a95534b2186cadb298381f8b39162650e5c1e` |

Current proven Krea 2 Turbo + 4x upscaler workflow lane:

| Field | Value |
|---|---|
| Outcome class / model | `image.heavy.le17mp` |
| Endpoint family | `mayhem_comfy_workflows` |
| Endpoint | `POST /v1/workflows` |
| Enclave ID | `997a76256af8236e32c06ccc2d615c625b208ee435c798e73de0d738334e41f2` |
| Canonical room | `5a5961a6ef3fa0c67db5b4e75c8e0566` |
| Workflow class artifact root | `13b11b153e7cdadf973e2efbe7f3269f43656b375917005875f130f089ec2aea` |
| Workflow class source SHA-256 | `58db2ca0a564c200ebf6635544c57d6f1d9970df9e759b05d2b9c49ce14fb4d1` |
| Required inventory root | `f58f46401fcec0a446d366daf43ce9a1318bbc4a1e00c1ace78a4a441bafe34a` |
| Runtime | `comfyui-v0.30.1` |
| Pricing unit | `megapixel_step` |
| 1024x1024, 8-step, 4x output usage | `136` `megapixel_step` |
| Paid proof | `.42` provider through `.31` sponsored fiat gateway; session `36c5476892db7056be5ddf36dcc09436ee7a44a036ed9b4b70a2877276f7e2b9`; artifact `openmayhem-krea-4x-mercedes-salespitch-paid-v0.2.123.png`; BLAKE3 `ec80feb418f369aa509020ecad96cd6886fb8b6af6fde617a2b367a2177b872a` |

Krea 2 Turbo itself is the base image generator. A true 4x result is a single
Comfy workflow graph that runs Krea and then an approved upscaler node. That is
not the same market as the base lane: publish it under the `image.heavy.le17mp`
outcome class, include `UpscaleModelLoader` and `ImageUpscaleWithModel` in the
signed workflow policy, and add a signed upscaler part with `scale: 4`.
Recommended first upscaler is `4x-spanx4-ch48.safetensors` because it is small,
Apache-2.0, and signed for all lanes.

Current standalone 4x upscaler workflow lane:

| Field | Value |
|---|---|
| Outcome class / model | `upscale.conv.le24mp` |
| Endpoint family | `mayhem_comfy_workflows` |
| Endpoint | `POST /v1/workflows` |
| Workflow class artifact root | `21548409983411023a80bd58beec92c2b6c2b43095576e1db64f61e61098d466` |
| Workflow class source SHA-256 | `95010acb441fee196d7c617a9f07a360074cfe0e6c0e1e329861e981afbe5291` |
| Required inventory root | `e3939d8c9af52b8772faaa0c2570e27d3cb3f963001c09b8efe42f6e48f42c98` |
| Runtime | `comfyui-v0.30.1` |
| Pricing unit | `megapixel` |
| Required part | `4x-spanx4-ch48.safetensors` / `d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a` |
| Reference proof | Local admission proof passed with graph SHA-256 `9a24896bfe8a78bcac41f7895126ad02593bde5bcabb9aec70229c7940d2f2a2`; output `openmayhem-upscale-conv-le24mp-reference-v0.2.127.png`; SHA-256 `3b78c0ecd45cfa63c75d2eea18c7056c417015908c7ec7c8dddc327949e4f8fc` |

Current SeedVR2 diffusion upscale workflow lane:

| Field | Value |
|---|---|
| Outcome class / model | `upscale.diffusion` |
| Endpoint family | `mayhem_comfy_workflows` |
| Endpoint | `POST /v1/workflows` |
| Required inventory root | `f501d4d7340fe2d891560aef0192adb28c0bd8e91c77b36e4ccc0e16b33fd15b` |
| Runtime | `comfyui-v0.30.1` |
| Pricing unit | `megapixel_step` |
| Required parts | SeedVR2 3B int8 convrot `e2a27b04c8c7244829fc5fbe3281cf7d29c7f65ef315fbb97386a66e2b3da7c7`; SeedVR2 VAE `63e6908333939636708d0661208d534237a117d1a6a36f4c3544c1cff40be6a1` |
| Reference proof | `.70` admission proof passed with graph SHA-256 `2f8f41767f90fd18a7a7109b156ef2464f9c22648ce37ada3126e02e76fc2c93`; output `openmayhem-seedvr2-upscale-diffusion-reference-v0.2.136.png`; SHA-256 `671c24b35e99ee28edbf08b3c88c37472fbb552dfd52764f9f2d6f422149e328`; canary hash `fe8181818181817f` |
| Product state | Signed dev policy, reference admission, and paid fiat `/v1/workflows` route proof passed; retained artifact `openmayhem-seedvr2-upscale-diffusion-paid-v0.2.138.png`, session `35b01e56d29b1fe11c1dd1b73fe7eca055ff35825094c7302f2a47c158bb3101`, BLAKE3 `22895752f732ddfeb17f3aaa93fbad6b3e2bf60a9e7b9c93c7bcadd3140d1dfb`, SHA-256 `e23ce117ba5325919c642a531f69cea09dd9b36a0c27fdcb41ca14790730d9ff`, `256x256` PNG, usage `1` `megapixel_step`; product review pending |

Krea providers must add these signed parts before admission:

| Part | Part ID | Purpose |
|---|---|---|
| `krea2_turbo_fp8_scaled.safetensors` | `6335241281bfe4537bda70cab1aca27211a9afb14197740c16778a253836bdae` | Krea 2 Turbo checkpoint, [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/e23f4757c6e570abca40b1c35e08e3a1229d591d/records/6335241281bfe4537bda70cab1aca27211a9afb14197740c16778a253836bdae.json) |
| `qwen3vl_4b_fp8_scaled.safetensors` | `19d454e5e0516af43d0a6aee3aefd468897851bd879add036fe1b9350b66825c` | Krea 2 text encoder, [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/e23f4757c6e570abca40b1c35e08e3a1229d591d/records/19d454e5e0516af43d0a6aee3aefd468897851bd879add036fe1b9350b66825c.json) |
| `qwen_image_vae.safetensors` | `106d81a4897fa125d63b62fbcf2d7d1e88dc66f1b89e6f793f7142f928c7aa70` | Krea/Qwen image VAE, [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/e23f4757c6e570abca40b1c35e08e3a1229d591d/records/106d81a4897fa125d63b62fbcf2d7d1e88dc66f1b89e6f793f7142f928c7aa70.json) |
| `4x-spanx4-ch48.safetensors` | `d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a` | Optional Krea+4x upscaler part with signed `scale: 4`, [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/e23f4757c6e570abca40b1c35e08e3a1229d591d/records/d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a.json) |

### Current Comfy parts inventory

The authoritative Comfy workflow reference is
[`COMFY-CHEATSHEET.md`](COMFY-CHEATSHEET.md). It lists every workflow outcome
class, the class-fit matrix, required Krea/LTX/H3/lipsync part sets, provider
commands, user API shape, and all 96 signed parts.

Current signed parts anchor:

- Dataset: `TracNetwork/openmayhem-parts-index`
- Revision: `36a1ce2720ff963f2f58555a2998d8035138932f`
- Index root: `7cd414ac0fb297bb325f8db51324ae4b58b242ed8289d160eeb1313f395f3a13`
- Anchor hash: `be3dab174f63c21b36dfded85ce9525d56e675c5dc6399237e845241deec2236`
- Index version: `13`
- Inventory: 96 parts: 1 audio-model, 12 checkpoint, 5 video-model, 5 text-encoder,
  8 VAE, 1 CLIP-vision, 2 LoRA, 7 lipsync, 35 ControlNet/control helper, and
  20 upscaler/restoration parts.
- Index URL: https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/36a1ce2720ff963f2f58555a2998d8035138932f/index.json
- Anchor URL: https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/36a1ce2720ff963f2f58555a2998d8035138932f/anchor.json

Every Comfy calibration must list every file loaded by the reference graph in
the signed workflow policy. Missing checkpoints, encoders, VAEs, LoRAs,
ControlNets, upscalers, lipsync models, or helper models must be mirrored and
signed as parts before the proof counts. Manual out-of-policy downloads are not
OpenMayhem calibration evidence.

The final proof must also pass the Comfy acceptance gate in
[`COMFY-CHEATSHEET.md`](COMFY-CHEATSHEET.md): signed inventory, blessed runtime
nodes, embedded workflow policy, human-inspected quality, and a paid
`/v1/workflows` request. A valid container, frame count, or waveform alone is
not sufficient if the media does not match the requested task.

### Rooms, payments, and limits

- `--rooms auto` joins existing canonical rooms. Providers cannot create a
  contract-canonical room.
- A route is usable only where buyer and provider rails intersect and the
  provider has a current verified payout binding for that rail.
- Providers select rails and bind their own payout targets:

  ```bash
  mayhem provider rails set --rails fiat,tap,tnk --submit
  mayhem provider payout set --rail tap --submit
  mayhem provider payout set --rail tnk --submit
  mayhem provider payout get
  mayhem provider stripe onboard --country <CC>
  ```

- `mayhem provider stripe adopt --country <CC>` adopts an existing eligible
  Stripe Standard account. A terminal always prints a copyable URL even when it
  cannot open a browser. The same provider identity reuses its bindings across
  models and rooms; another provider identity uses the signed relink flow.
- A first payout binding activates at the next epoch; a later rotation activates
  at epoch E+2. The provider submits the signed intent through its read-only
  peer; the sole canonical indexer verifies and appends it.
- Provider min-ask, utilization, concurrency, and local safety limits remain
  provider controls. Canonical starting prices, brackets, fees, epochs, models,
  enclaves, and rooms remain admin controls.

## Runtime lock summary

These are release-managed runtime identities, not commands for manual package
installation or claims that the listed versions are universally required
outside OpenMayhem. They are the versions bound to the current measured
runtime/canary evidence.
Where a platform `uv.lock` exists, it is the authoritative transitive lock.
The older vLLM, MLX language/vision, Transformers ASR, and Sulphur CUDA
manifests instead pin and validate their direct runtime stack; their remaining
dependency closure is resolver-constrained rather than hash-locked. In every
case the signed catalog, runtime selector, imports/version checks, and
functional canary remain the admission authority.

Every managed Python backend starts from the official standalone `uv 0.11.29`
archive for its exact OS and architecture. Mayhem enforces HTTPS, redirect and
size bounds, pinned archive and executable SHA-256 values, safe extraction, and
atomic activation before using it to materialize a frozen runtime.

| Backend | Exact managed runtime |
|---|---|
| vLLM | `uv 0.11.29`, Python 3.12, `vllm 0.24.0`, `torch 2.11.0`, `transformers 5.12.1`, `tokenizers 0.22.2`, `safetensors 0.8.0`, `compressed-tensors 0.17.0`, `triton 3.6.0`, `av 18` |
| MLX language/vision | `uv 0.11.29`, Python 3.12, `mlx-lm 0.31.3`, `mlx-vlm 0.6.3`, `mlx 0.32.0`, `llguidance 1.7.6`, `transformers 5.12.1`, `tokenizers 0.22.2`, `safetensors 0.8.0`, `av 18` |
| llama.cpp media | `llama-cpp-2`/`llama-cpp-sys-2 0.1.150` from llama.cpp revision `7f15e87e3cb0f636e236243e6ee4fc2a4c357277`, with `llguidance` and `mtmd`; acceleration is compile-time CUDA, Metal, or Vulkan. macOS source builds use Metal with runtime CPU fallback; Linux x86_64/aarch64 and Windows x86_64 select working CUDA, then Vulkan, then CPU; Windows ARM64 uses CPU. `MAYHEM_LLAMA_CPP_FEATURES` preserves an explicit, validated operator override. |
| Transformers ASR | `uv 0.11.29`, Python 3.12, `transformers 5.14.1`, `torch 2.13.0`, `tokenizers 0.22.2`, `safetensors 0.8.0`, `numpy 2.4.6`, `soundfile 0.14.0`, `soxr 1.1.0`, `librosa 0.11.0` |
| ACE-Step | Embedded ACE-Step `0.1.8` source revision `dce621408bee8c31b4fcf4811682eb9359e1bc94` (package `ace-step 1.5.0`, source archive SHA-256 `816a58b7cdc66b3817625dd67e7407b77c0d05e8526a70f6a43cd93889655080`, lock SHA-256 `0a9c8067b3299bfc6881a06e097ff95e55e1b7bb8f9d1f84192ac23e59b995ab`); direct locks include `accelerate 1.12.0`, `diffusers 0.37.1`, `transformers 4.57.6`, `tokenizers 0.22.2`, `safetensors 0.7.0`, `soundfile 0.13.1`, and `av 18.0.0`. Platform Torch is Windows `2.7.1+cu128`, Linux x86_64 `2.10.0+cu128`, Linux ARM64 `2.10.0+cu130`, or macOS ARM64 `2.10.0`; matching `torchaudio` and `torchvision` are lock-resolved. |
| Sulphur CUDA | `diffusers 0.39.0`, `torch 2.9.1`, `torchvision 0.24.1`, `transformers 4.57.6`, `tokenizers 0.22.2`, `accelerate 1.12.0`, `bitsandbytes 0.49.1`, `peft 0.18.1`, `safetensors 0.8.0`, `gguf 0.19.0`, `huggingface-hub 0.36.0`, `av 16.1.0`, `numpy 2.2.6`, `Pillow 12.1.0`, `tqdm 4.67.1`; CUDA 13.0 wheel family |
| Sulphur MLX | Embedded LTX core/pipelines `0.14.19` from revision `e1838a855bfd1640135c424c96cb27a0c0ad150e`; `mlx`, `mlx-lm`, and `mlx-metal 0.31.1`, `transformers 5.3.0`, `tokenizers 0.22.2`, `safetensors 0.7.0`, `numpy 2.4.3`, `Pillow 12.1.1` |
| Chatterbox | Python 3.11; `chatterbox-tts 0.1.7`, `conformer 0.3.2`, `diffusers 0.29.0`, `gradio 6.8.0`, `librosa 0.11.0`, `numpy 1.26.4`, `omegaconf 2.3.0`, `pykakasi 2.3.0`, `pyloudnorm 0.2.0`, Perth revision `ce86c49d029f42272c1902eccb675556b9ed2330`, `s3tokenizer 0.3.0`, `safetensors 0.5.3`, `spacy-pkuseg 1.0.1`, `transformers 5.2.0`; CPU/MPS and x86 CUDA retain their frozen platform flavors, while Linux/aarch64 CUDA uses `torch 2.9.1+cu130`, `torchaudio 2.9.1`, and CUDA 13.0 from its separate hash-pinned lock |
| Needle | Model `Cactus-Compute/needle@5f89b4307696d669c3df1d38ae057e6e1728b107`; runtime source `Cactus-Compute/needle-hf@ffd0d081401257fee31150d30c494b2f98910fc0`; exact hash-pinned release locks are split by CPU and CUDA platform. Apple MPS is measured but intentionally not market-eligible. |

The authoritative full locks are
[`python_runtime.rs`](crates/mayhem-cli/src/python_runtime.rs),
[`sulphur-runtime-requirements.txt`](crates/mayhem-cli/resources/python/sulphur-runtime-requirements.txt),
[`sulphur-mlx-runtime-requirements.txt`](crates/mayhem-cli/resources/python/sulphur-mlx-runtime-requirements.txt),
and the four
[`chatterbox-runtime-*`](crates/mayhem-cli/resources/python/) lock
directories. Stable-diffusion.cpp is the one external engine below: its
executable version is not release-pinned, so this document does not invent one.

## Canonical catalog at a glance

| Exact model ID | Class | Canonical backend/artifact | Tier floor/fallback | Catalog RAM | Full-offload guidance | Download |
|---|---|---|---|---:|---:|---:|
| `Qwen/Qwen3.8-27B` | Text generation | vLLM / NVFP4 | Tier 1 floor; Tier-1/Tier-2 markets published | 48 GiB | 24 GiB NVIDIA | 23,114,056,343 B (21.52 GiB) |
| `hauhaucs/qwen3.6-35b-a3b-uncensored` | Text generation | vLLM / NVFP4 | Tier 1; use highest proved tier | 48 GiB | 24 GiB NVIDIA | 23,374,279,873 B (21.77 GiB) |
| `google/gemma-4-E4B-it` | Text generation | llama.cpp / Q4_K_M GGUF | Tier 1; use highest proved tier; cataloged, not owned-fleet served | 12 GiB | 8 GiB | 6,326,841,504 B (5.89 GiB) |
| `tongyi/z-image-turbo` | Image generation | stable-diffusion.cpp / Q4_K GGUF | Tier 1; use highest proved tier | 16 GiB | 8 GiB | 6,696,835,812 B (6.24 GiB) |
| `nvidia/parakeet-tdt-0.6b-v3` | Speech to text | Transformers safetensors | Tier 1; use highest proved tier | 8 GiB | 4 GiB | 2,509,473,204 B (2.34 GiB) |
| `acestep/ace-step-1.5` | Music generation | ACE-Step safetensors composite | Tier 1; use highest proved tier | 16 GiB | 20 GiB | 10,092,101,191 B (9.40 GiB) |
| `prism-ml/Ternary-Bonsai-27B` | Text generation | llama.cpp Q2_0 or MLX 2-bit | Tier 1; use highest proved tier | 16 GiB | 8 GiB | 7.26 GiB GGUF / 7.94 GiB MLX |
| `SulphurAI/Sulphur-2-base` | Video generation | CUDA GGUF composition or MLX Q4 | Tier 1; use highest proved tier | 64 GiB | See measured guidance | 40.66 GiB CUDA / 44.32 GiB MLX |
| `ResembleAI/chatterbox` | Text to speech | PyTorch safetensors | Tier 1; use highest proved tier | 8 GiB | 6 GiB | 3,191,966,992 B (2.97 GiB) |
| `huihui-ai/Huihui-Agents-A1-abliterated` | Text generation | llama.cpp Q4_K GGUF | Tier 1; use highest proved tier | 32 GiB | 32 GiB | 22,069,579,360 B (20.55 GiB) |
| `Cactus-Compute/needle` | Deterministic tool selection | `needle-cpu` or CUDA-only `needle-gpu` | Tier 1; use highest proved tier | Managed preflight | See measured guidance | 30.4M parameters |
| `video.minimax_h3.t2v_i2v` | Comfy workflow video+audio | ComfyUI / signed MiniMax H3 parts plus optional SeedVR2 2x branch | Tier 1; live `.70` CUDA proof; paid dialogue-fight artifact `openmayhem-minimax-h3-anime-dialogue-fight-paid-v0.2.123.mp4`; optional 2x proof retained as `openmayhem-h3-seedvr2-2x-1792x1024-compare.mp4` | 96 GiB | 48 GiB base; add SeedVR2 headroom for branch | Four signed base parts, 42.29 GiB total; branch-capable inventory also needs SeedVR2 `e2a27b04...da7c7` and `63e69083...0be6a1`, root `1165f3bb28092852c60cdc61d524bb280a45eca39017147484adf9e1d9816ec6` |
| `video.minimax_h3.r2v` | Comfy workflow reference-media video+audio | ComfyUI / signed MiniMax H3 REF2VA parts | Tier 1; live `.70` admission and paid fiat proof `openmayhem-minimax-h3-r2v-paid-proof-v0.2.126.mp4` | 96 GiB | 48 GiB | Four signed R2V parts, 39.55 GiB payload |
| `video.minimax_h3.lowvram_t2v_i2v` | Comfy workflow video+audio | ComfyUI / submitted low-VRAM H3 pack: W4A8 diffusion, INT8 video VAE, turbo LoRAs, MiniMaxH3-Easy, KJ low-VRAM nodes | Source-ready; Windows CUDA reference proofs passed for T2V 4-step, T2V 6-step, and I2V 8-step on `comfyui-v0.32.0`; public serving is not live until release/catalog publication and paid route proof | 48 GiB target | 16 GiB VRAM + 32 GiB RAM target, not yet proven on a 16 GB VRAM card | Default `4` steps; user-adjustable only to `4`, `6`, or `8`; 736x1280, 10s, 243 frames; do not use 20-step H3 settings here |
| `video.minimax_h3.lowvram_r2v` | Comfy workflow reference-media video+audio | ComfyUI / submitted low-VRAM H3 R2V pack: W4A8 REF2VA, INT8 video VAE, 4-step REF2V LoRA, same custom nodes | Source-ready; Windows CUDA R2V 4-step reference proof passed on `comfyui-v0.32.0` with inventory root `cd8657f770f09a4fe7baaaf9a8cc2c42560c5282279bae1512fed30467f2d0dc`; public serving is not live until release/catalog publication and paid route proof | 48 GiB target | 16 GiB VRAM + 32 GiB RAM target, not yet proven on a 16 GB VRAM card | Submitted pack has only a 4-step R2V graph; do not advertise 6/8-step R2V until separately proven |
| `upscale.conv.le24mp` | Comfy workflow standalone 4x image upscale | ComfyUI / signed SPANx4 upscaler | Reference admission and paid route proof passed | 16 GiB | 4 GiB | One signed upscaler part, 8.6 MiB |
| `upscale.diffusion` | Comfy workflow SeedVR2 diffusion upscale | ComfyUI / signed SeedVR2 int8 + VAE parts | Reference admission and paid route proof passed; product review pending | 32 GiB | 10 GiB | Two signed parts, 3.69 GiB payload |

The RAM and full-offload columns are catalog admission/guidance fields. Model
weights, runtime environments, caches, outputs, and build artifacts require
additional disk and memory headroom.

## Qwen 3.8 27B NVFP4

Current status: signed catalog entry, `.29` technical calibration, paid FIAT
receipts, retained paid launch proof, designated persistent `.29` provider,
website billing proof, and the full `0.2.164` fleet rollout are complete.

**Selector and source**

- Model: `Qwen/Qwen3.8-27B`; operational Mayhem minimum `0.2.161`. Catalog
  version 51 publishes that minimum; `v0.2.164` is the completed fleet
  checkpoint, not the minimum client/provider version.
- Canonical provenance:
  `Qwen/Qwen3.8-27B@1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0`.
- Backend/artifact: `vllm` / `nvfp4`. The approved upstream artifact is
  `HivenetQuant/Qwen3.8-27B-NVFP4@cd5a8f0739c1df89d8cd9d39ede58c619d8298c2`;
  the canonical byte mirror is
  `TracNetwork/mayhem-catalog-Qwen-Qwen3-8-27B-NVFP4@4ee2ca6a5987ba7f07cbe0e779f73f98d66b4a94`.
- Artifact root:
  `36abadf4a7aa1ac3b60abc57bda718c3329cf3ad69fb9dfca13a5384c32c6f11`.
  The primary shard plus 17 signed weight/runtime sidecars total exactly
  23,114,056,343 bytes. Only complete hash- and Merkle-verified safetensors
  from the canonical mirror are eligible.
- Canary:
  [`canary-qwen3.8-27b-nvfp4-v1.json`](catalog/canaries/canary-qwen3.8-27b-nvfp4-v1.json),
  `token_fingerprint`, `match_min=0.9`, with 14 cases.

**Hard requirements and surface**

- Linux NVIDIA Blackwell is the documented serving path; compute capability
  must be at least 12.0. Catalog floors are 48 GiB RAM, 24 GiB NVIDIA dedicated
  or unified memory, and AVX2 or NEON. CPU, Apple Metal, AMD, pre-Blackwell
  NVIDIA, Windows, MLX, GGUF, and alternate quants are not eligible.
- The managed runtime is vLLM `0.24.0` with BF16 compute and artifact-scoped
  FP8 KV. The native context ceiling is exactly 262,144 tokens; there is no
  YaRN extension. Providers may advertise a smaller fitting native context,
  but only the `le8k`, `le32k`, `le128k`, and `le256k` brackets apply through
  this ceiling.
- The catalog reference rate is `$0.05` per million input tokens and `$0.35`
  per million output tokens. Current bracket prices and routes come only from
  the live ledger; the reference rate does not prove their publication.
- Endpoints are OpenAI chat completions, completions, responses, and HF
  multimodal chat. Input is text, image, or video; output is text. JSON, tools,
  streaming, `thinking_mode`, `thinking_history`, and low/medium/xhigh
  `reasoning_effort` are calibrated.

**Retained `.29` measurements (non-normative)**

- Two text-only requests genuinely overlapped at full 262,144-token context
  with vLLM scheduler capacity `2`, BF16 compute, FP8 KV, and
  `max_num_batched_tokens=2048`. Proof SHA-256:
  `3fbab1e8f6fed5d8b5e393e958edebef20544346d07ae66a0d68a7c8e59114fd`.
- The 1-megapixel image and 16-frame video calibration used NVIDIA unified
  memory with a 15% (`17.35 GiB`) reserve and a `98.32 GiB` F13 budget.
  Process-tree RSS was about `5.435 GiB`; measured working sets were `12 MiB`
  for image and `9.19 MiB` for video. RSS excludes accelerator allocations and
  does not replace the catalog admission floors.
- A warm paid streaming request on `.29` under release `0.2.162` processed
  12,017 prompt and 32 completion tokens. First content arrived at 7,785.77 ms
  and last content at 21,997.83 ms: 1,543.46 prompt tok/s end to end
  (1,626.19 tok/s after response headers) and 2.18 generation tok/s. The
  EngineCore held 46,303 MiB of accelerator memory, about 64.67 GiB of system
  memory remained available, and the provider had zero restarts. The request
  returned a final signed FIAT receipt. These are `.29` request-path results,
  not portable minima or pure kernel benchmarks. Separate paid launch proof
  used two overlapping full-context FIAT requests with independent final
  receipts; cancelling one shorter overlap did not disrupt its paid peer.

**Independent dispatch and capacity**

`independent_dispatch` is opt-in through the signed generation execution
profile for this exact artifact root, and currently covers only the exact
text-only modality set. Image/video requests remain exclusive. Without a
matching profile, vLLM serving remains serial. For an opted-in provider,
Mayhem derives context-dependent capacity from hwprobe, the provider/operator
session limit, usable memory after reserves and claims, per-session KV memory,
any signed scheduler ceiling, and vLLM's runtime KV capacity; it advertises the
derived maximum and active load in heartbeats. The `.29` value `2` is evidence
for `.29`, not a hardcoded canonical ceiling or a default for other machines.

**Start**

```bash
mayhem doctor --provider-backend vllm
mayhem up --provider --provider-enclave Qwen/Qwen3.8-27B --yes
```

Do not substitute another Qwen checkpoint, quantization, runtime, or local
weights. A green local start is not sufficient proof for a new provider;
verify its selected rails and payout bindings, route visibility, paid request,
usage, and final receipt.

## Qwen 3.6 35B-A3B uncensored

**Selector and source**

- Model: `hauhaucs/qwen3.6-35b-a3b-uncensored`
- Backend/artifact: `vllm` / `nvfp4`
- Admin mirror:
  `TracNetwork/mayhem-catalog-hauhaucs-qwen3-6-35b-a3b-uncensored-NVFP4@58722d97ba2d93c32740f409efc9155b784edb95`
- Primary `model.safetensors`: 23,354,242,416 bytes, plus eight signed
  configuration, tokenizer, processor, template, and recipe sidecars.
- Canary:
  [`canary-launch-v2.json`](catalog/canaries/canary-launch-v2.json)

**Hard requirements and surface**

- Linux NVIDIA Blackwell is the current canonical path; compute capability must
  be at least 12.0. Windows, CPU, Apple Metal, AMD, and pre-Blackwell NVIDIA are
  unsupported for this artifact.
- 48 GiB RAM, 24 GiB NVIDIA dedicated or unified memory, and AVX2 or NEON.
- vLLM preflight requires a CUDA toolkit containing `bin/nvcc`; its managed
  bootstrap requires at least 8 GiB free disk.
- Endpoints: OpenAI chat completions, completions, responses, and HF multimodal.
  Input is text, image, or video; output is text. Streaming, JSON, tools, and
  multiple tool calls are supported.
- Context ceiling: 262,144 tokens; brackets `le8k`, `le32k`, `le128k`,
  `le256k`.
- Sampling ranges: temperature `0..2`, top-p `0.000001..1`, top-k
  `0..1000000`, min-p `0..1`, frequency/presence penalty `-2..2`, seed
  `0..4294967295`. Defaults are temperature `1`, top-p `0.95`, top-k `20`,
  min-p `0`, presence penalty `1.5`. `thinking_mode` is
  `enabled|disabled` (default enabled); `thinking_history` is
  `latest_only|preserve` (default latest only).

**Measured guidance**

- Signed modality evidence uses a 1-megapixel image and 16 video frames.
- Calibration process RSS was about 2.40 GiB, with roughly 12 MiB incremental
  image and 9 MiB incremental video working RSS. This excludes the meaningfully
  larger accelerator allocation and does not lower the 48/24 GiB admission
  floors.

**Start**

```bash
mayhem doctor --provider-backend vllm
mayhem up --provider --provider-enclave hauhaucs/qwen3.6-35b-a3b-uncensored --yes
```

Do not substitute another Qwen checkpoint, quantization, or CPU fallback.

## Gemma 4 E4B IT

Owned-fleet status: cataloged only. Do not count Gemma as an owned live route
unless a provider is explicitly running it.

**Selector and source**

- Model: `google/gemma-4-E4B-it`
- Backend/artifact: `llama.cpp` / `gguf-q4_k_m`
- Admin mirror:
  `TracNetwork/mayhem-catalog-google-gemma-4-E4B-it-GGUF@68772908c9431af9c9bfc3cee0ebefcd74995891`
- Upstream pin:
  `lmstudio-community/gemma-4-E4B-it-GGUF@53a691ddc52708042c56f80cdaf47f8a1daf051e`
- Primary GGUF: 5,335,289,664 bytes. Mandatory BF16 multimodal projector:
  991,551,840 bytes.
- Canary:
  [`canary-gemma4-launch-v1.json`](catalog/canaries/canary-gemma4-launch-v1.json)

**Hard requirements and surface**

- Linux, Windows, or macOS CPU; CUDA, Metal, or Vulkan only when compiled into
  the installed Mayhem build.
- 12 GiB RAM and AVX2 or NEON. The 8 GiB VRAM value is a full-offload target,
  not a CPU admission minimum.
- Endpoints: OpenAI chat completions, completions, responses, and HF multimodal.
  Text, image, audio, and video input produce text; JSON and tools are supported.
- Context ceiling: 131,072 tokens; brackets `le8k`, `le32k`, `le128k`.
- Signed audio input is WAV up to 30 seconds. Signed video input is 1 fps up to
  60 frames.
- Defaults: temperature `1`, top-p `0.95`, top-k `64`;
  `thinking_mode=disabled`. `visual_token_budget` is exactly
  `budget_70|budget_140|budget_280|budget_560|budget_1120`, default
  `budget_280`.

**Measured guidance**

- Calibration dedicated-memory baselines were about 4.49-4.88 GiB. Peaks were
  about 7.20-7.61 GiB for the 1-megapixel image, 30-second audio, and 60-frame
  video cases. These are proof-host observations, not portable guarantees.

**Start**

```bash
mayhem doctor --provider-backend llama.cpp
mayhem up --provider --provider-enclave google/gemma-4-E4B-it --yes
```

The signed BF16 projector is mandatory. Do not pair the GGUF with another
projector.

## Z-Image Turbo

**Selector and source**

- Model: `tongyi/z-image-turbo`
- Backend/artifact: `stable-diffusion.cpp` / `gguf-q4_k`
- Admin mirror:
  `TracNetwork/mayhem-catalog-tongyi-z-image-turbo-GGUF@b0110258385798d6e5b9bea626f6560607ce17ad`
- Upstream pin:
  `leejet/Z-Image-Turbo-GGUF@c61c0e422dc8b541b7548cf33a4ef8302b0f8085`
- Primary GGUF: 3,864,250,304 bytes. Mandatory Qwen text encoder:
  2,497,281,120 bytes. Mandatory VAE: 335,304,388 bytes.
- Canary:
  [`canary-z-image-launch-v1.json`](catalog/canaries/canary-z-image-launch-v1.json)

**Hard requirements and surface**

- Linux, Windows, or macOS CPU; CUDA, Metal, ROCm, or Vulkan when the matching
  external stable-diffusion.cpp build is installed.
- 16 GiB RAM. The 8 GiB VRAM value is a full-offload target.
- `sd-cli` must be on `PATH` or named by
  `MAYHEM_STABLE_DIFFUSION_CPP_BIN`; a sibling `sd-server` is required. Mayhem
  pins and verifies model files but does not pin or install that external
  executable.
- Endpoints: OpenAI image generations and HF text-to-image.
- Prompt length `1..32000`; optional negative prompt `0..32000`; `n=1..4`;
  `response_format=b64_json`; width/height `576..2048`, each divisible by 16,
  default `1024x1024`; steps `7..9`, default `9`; guidance `0..49`, default
  `0`; shift `1..10`, default `3`; seed `0..4294967295`, default `42`.
- Signed adapter semantics map public steps by `-1` and guidance by `+1`.
  Providers must not alter those offsets.

**Measured guidance**

- The 1-megapixel canary process RSS rose from about 294 MiB to 683 MiB.
  Model weights and accelerator allocations are additional.
- One image generation is admitted in flight per calibrated worker.

**Start**

```bash
mayhem doctor --provider-backend stable-diffusion.cpp
mayhem up --provider --provider-enclave tongyi/z-image-turbo --yes
```

Do not download an arbitrary engine binary or substitute the signed text encoder
or VAE.

## Parakeet TDT 0.6B v3

**Selector and source**

- Model: `nvidia/parakeet-tdt-0.6b-v3`
- Backend/artifact: `transformers-asr` / safetensors
- Admin mirror:
  `TracNetwork/mayhem-catalog-nvidia-parakeet-tdt-0-6b-v3-Transformers@a83f71f1a8a1cf099b5dbe23262c5028ad931086`
- Upstream pin:
  `nvidia/parakeet-tdt-0.6b-v3@7c35754d166cca382ad1e53e68b01e7c575f3a1d`
- Primary weights: 2,508,311,120 bytes plus five signed processor, tokenizer,
  and configuration sidecars.
- Canary:
  [`canary-stt-launch-v2.json`](catalog/canaries/canary-stt-launch-v2.json)

**Hard requirements and surface**

- Linux, Windows, or macOS CPU; CUDA on Linux/Windows; Metal/MPS on macOS.
- 8 GiB RAM and AVX2 or NEON. The 4 GiB VRAM value is a full-offload target.
- Managed runtime bootstrap needs 8 GiB free disk.
- Endpoints: OpenAI audio transcription and HF ASR.
- Input is bounded 16 kHz mono WAV or FLAC. Automatic recognition covers 25
  signed languages, punctuation/capitalization, overlap-chunked long audio,
  and word/segment timestamps.
- OpenAI formats:
  `json|text|srt|verbose_json|vtt`. HF timestamps:
  `false|true|word|segment`.
- Forced language, prompt conditioning, sampling controls, and streaming
  transcription are unsupported. Concurrency is one transcription.

**Measured guidance**

- The signed long-audio proof is 130 seconds.
- Calibration process RSS rose by roughly 24 MiB; loaded weights and backend
  allocations are additional.

**Start**

```bash
mayhem doctor --provider-backend transformers-asr
mayhem up --provider --provider-enclave nvidia/parakeet-tdt-0.6b-v3 --yes
```

The portable provider artifact is the signed Transformers mirror, not an
upstream NeMo checkout.

## ACE-Step 1.5

**Selector and source**

- Model: `acestep/ace-step-1.5`
- Backend/artifact: `ace-step` / signed safetensors composite
- Admin mirror:
  `TracNetwork/mayhem-catalog-ACE-Step-Ace-Step1-5-SFT@f41443d7171a03181ada08912780b0449e8ff7fe`
- Upstream SFT pin:
  `ACE-Step/acestep-v15-sft@c410d249e71ea9385a7b586865e65b1473e1098d`;
  all embedding, language-model, VAE, code, and latent components have their own
  signed pins.
- Primary DiT: 4,787,825,604 bytes. Total primary plus 25 sidecars:
  10,092,101,191 bytes.
- Canary:
  [`canary-music-launch-v1.json`](catalog/canaries/canary-music-launch-v1.json)

**Hard requirements and surface**

- CPU: Linux x86_64/ARM64, Windows x86_64, macOS x86_64/ARM64. CUDA:
  Linux/Windows. Metal/MPS: Apple Silicon.
- 16 GiB RAM, 20 GiB VRAM for full offload, AVX2 or NEON. Current preflight
  permits CUDA CPU/INT8 partial offload from 4 GiB usable VRAM and MPS from
  16 GiB available unified memory. Runtime bootstrap needs 24 GiB free disk.
- Endpoints: OpenAI music generation, OpenAI audio generation, and HF
  text-to-audio.
- Full music modes:
  `text2music|cover|cover-nofsq|repaint`. Prompt/caption composed maximum is 512
  characters; lyrics maximum is 4,096. Source/reference input accepts signed
  AAC, FLAC, M4A, MP4, MPEG, MP3, OGG, Opus, and WAV.
- Duration is auto/`-1` or `10..600` seconds; steps `1..200` (default 50);
  guidance `1..15` (default 7); `n=1..8` (default 2); seed
  `-1..4294967295`; BPM `30..300`. Key, time signature, ODE/SDE,
  Euler/Heun, cover/repaint, normalization, and fade controls are signed.
  Output is `flac|opus|aac|wav|wav32|mp3`.
- The simpler audio/HF families expose their narrower signed subset.
  Concurrency is one generation.

**Measured guidance**

- The launch canary produces 10 seconds of audio.
- Calibration process RSS rose from about 3.30 GiB to 4.41 GiB, an incremental
  1.11 GiB. Backend/device allocations still need headroom.

**Start**

```bash
mayhem doctor --provider-backend ace-step
mayhem up --provider --provider-enclave acestep/ace-step-1.5 --yes
```

The embedded source and managed runtime are part of the measured enclave. Do
not enable arbitrary remote code or replace individual composite components.

## Ternary Bonsai 27B

**Selector and source**

- Model: `prism-ml/Ternary-Bonsai-27B`
- GGUF variant: `llama.cpp` / `gguf-q2_0`; mirror
  `TracNetwork/mayhem-catalog-prism-ml-Ternary-Bonsai-27B-GGUF@49b3f8175fc1b066110dce26e3e76313b2c04d93`,
  upstream
  `prism-ml/Ternary-Bonsai-27B-gguf@abbae723028d71be674e71e1a71201a6f43fab22`.
  Primary: 7,165,121,600 bytes; Q8 projector: 629,246,880 bytes.
- MLX variant: `mlx` / `mlx-2bit`; mirror
  `TracNetwork/mayhem-catalog-prism-ml-Ternary-Bonsai-27B-MLX-2bit@2935ee5921feb4b0effddeedc68bf0b7babb419b`,
  upstream
  `prism-ml/Ternary-Bonsai-27B-mlx-2bit@70f75f3ad081ab840a42f3304c02c27e7f89bfb7`.
  Total: 8,521,049,516 bytes.
- Canary:
  [`canary-bonsai-launch-v1.json`](catalog/canaries/canary-bonsai-launch-v1.json)

**Hard requirements and surface**

- GGUF: Linux, Windows, or macOS CPU, plus a compiled CUDA, Metal, or Vulkan
  accelerator. MLX: Apple Silicon only.
- 16 GiB RAM, 8 GiB VRAM full-offload target, AVX2 or NEON. MLX bootstrap needs
  2 GiB free disk; llama.cpp media bootstrap needs 1 GiB.
- Endpoints: OpenAI chat completions, completions, responses, and HF
  multimodal. Input is text, image, or video; output is text. JSON and tools are
  supported.
- Context ceiling: 262,144 tokens; brackets `le8k`, `le32k`, `le128k`,
  `le256k`.
- Defaults: temperature `0.7`, top-p `0.95`, top-k `20`.
  `thinking_mode=enabled|disabled` (default enabled);
  `thinking_history=latest_only|preserve`.
- The DGX Spark path is intentionally unsupported because the required
  temperature-zero identity proof failed. Do not reinterpret that as a
  network-wide NVIDIA ban.

**Measured guidance**

- Signed image evidence uses 1 megapixel; signed video evidence uses 16 frames.
- MLX calibration RSS was about 7.98 GiB baseline and 8.02 GiB peak. GGUF
  process RSS was about 1.08-1.11 GiB baseline and 1.36 GiB peak; memory-mapped
  weights and device allocations make process RSS alone an incomplete sizing
  figure.
- One generation is admitted in flight.

**Start**

```bash
mayhem doctor --provider-backend llama.cpp
mayhem up --provider --provider-enclave prism-ml/Ternary-Bonsai-27B --yes
```

On Apple Silicon, select/preflight `mlx` instead when that is the intended
canonical artifact. Never mix a projector or primary across the two variants.

## Sulphur 2 base

**Selector and source**

- Model: `SulphurAI/Sulphur-2-base`
- CUDA variant: `sulphur` / `gguf-q4_k_m` composition; mirror
  `TracNetwork/mayhem-catalog-SulphurAI-Sulphur-2-base-GGUF@659728cebbcb4cc3c48f1ff3a6d237f4c4357aa6`,
  upstream
  `SulphurAI/Sulphur-2-base@875e886e556b955d21149316fd631cc121db6cc1`.
  Total: 43,657,590,667 bytes.
- MLX variant: catalog engine `sulphur` / `mlx-q4` composition; internal
  managed-runtime selector `sulphur-mlx`; mirror
  `TracNetwork/mayhem-catalog-SulphurAI-Sulphur-2-base-MLX-4bit@72d1f4293e8b7ac913618cc146653acea15845a4`,
  upstream
  `MLXBits/sulphur-2-distill-mlx-q4@d210a0937cac3464ef80c74806e886beddf19a8e`.
  Total: 47,589,019,748 bytes.
- Canary:
  [`canary-sulphur-calibration-v3.json`](catalog/canaries/canary-sulphur-calibration-v3.json)

**Hard requirements and surface**

- CUDA variant: Linux or Windows NVIDIA, compute capability at least 8.9.
  MLX variant: Apple Silicon. CPU-only serving is unsupported.
- 64 GiB RAM. The catalog does not invent a portable VRAM minimum; use the
  measured allocations below plus operating-system and runtime headroom.
- `ffmpeg` and `ffprobe` are required. Their exact detected versions are
  recorded in evidence, but no universal executable version is hard-pinned.
  Mayhem resolves both tools and checks the required codecs, demuxers, muxers,
  and version evidence before model load, with an actionable error before a
  long initialization begins.
- Endpoints: OpenAI video generation and HF text-to-video. Text-to-video and
  bounded image-conditioned video are supported, with synchronized audio.
- Prompt `1..4096`; negative prompt `0..4096` on CUDA/GGUF only (MLX requires
  it empty); width/height `256..2048`, divisible by 64; fps `1..50`; `n=1`;
  frames are `9 + 8k` through 497; duration `1..10` seconds; seed
  `0..4294967295`; up to 16 signed conditions; selectable prompt enhancer.
- The distilled schedule is fixed at 8 video steps plus 3 audio steps. There
  is no user step control. One generation is admitted in flight.

**Measured guidance**

- MLX proof: about 17.83 GiB baseline and 27.34 GiB peak for a 6-second,
  121-frame generation.
- CUDA proof: about 45.41 GiB baseline and 50.79 GiB peak.
- These are backend-specific calibration observations, not interchangeable
  admission floors or a claim that every prompt uses the same memory.

**Start**

```bash
mayhem doctor --provider-backend sulphur
mayhem up --provider --provider-enclave SulphurAI/Sulphur-2-base --yes
```

The catalog and doctor selector remain `sulphur`; Mayhem chooses its internal
managed-runtime selector `sulphur-mlx` for the Apple Silicon artifact. CUDA and
MLX are canonical variants in the same model market only when their signed
capability and canary bindings match; never cross-mix their sidecars or runtime
environments.

## Chatterbox original-English TTS

**Selector and source**

- Model: `ResembleAI/chatterbox`
- Backend/artifact: `chatterbox` / PyTorch safetensors
- Admin mirror:
  `TracNetwork/mayhem-catalog-ResembleAI-chatterbox-PyTorch@0adbad4d3515285bdcdc3d503759e7110e664201`
- Upstream pin:
  `ResembleAI/chatterbox@5bb1f6ee58e50c3b8d408bc82a6d3740c2db6e18`;
  embedded runtime source revision
  `59bc590b3cad826e5d5987745bf6844627a21ad5`; Perth watermark revision
  `ce86c49d029f42272c1902eccb675556b9ed2330`.
- Primary: 2,129,653,744 bytes. Mandatory `ve.safetensors`,
  `s3gen.safetensors`, `tokenizer.json`, and `conds.pt` bring the total to
  3,191,966,992 bytes.
- Canary:
  [`canary-chatterbox-launch-v1.json`](catalog/canaries/canary-chatterbox-launch-v1.json)

**Hard requirements and surface**

- Linux/Windows CPU; CUDA on Linux/Windows x86_64; CPU or Metal/MPS on Apple
  Silicon. Linux ARM64 supports the frozen CUDA 13 runtime and falls back to
  the frozen CPU runtime when CUDA is unavailable. Intel macOS and Windows
  ARM64 CUDA are unsupported by the current managed runtime selector.
- 8 GiB RAM; 6 GiB full-offload target. Runtime bootstrap needs 16 GiB free
  disk.
- Endpoints: OpenAI audio speech and HF text-to-speech. Output is mono 24 kHz
  WAV with Perth watermarking.
- Input length `1..16384`; `voice` is exactly `default`. Zero-shot voice cloning
  uses a bounded base64 WAV `reference_audio`; it replaces the need for a
  built-in voice library.
- Controls: exaggeration `0.25..2` (default 0.5), cfg weight `0..1` (0.5),
  temperature `0.05..5` (0.8), min-p `0..1` (0.05), top-p `0..1` (1),
  repetition penalty `1..2` (1.2), seed `0..4294967295` (7).
- One synthesis is admitted in flight.

**Measured guidance**

- The signed launch clip is 7 seconds.
- Calibration RSS rose from about 1.48 GiB to 2.84 GiB, an incremental
  1.36 GiB, excluding other device/runtime headroom.

**Start**

```bash
mayhem doctor --provider-backend chatterbox
mayhem up --provider --provider-enclave ResembleAI/chatterbox --yes
```

Do not substitute the multilingual or turbo checkpoint or invent named voices.
An explicit CUDA request still fails closed when the matching frozen platform
runtime or a usable NVIDIA device is absent.

## Huihui Agents A1 abliterated

**Selector and source**

- Model: `huihui-ai/Huihui-Agents-A1-abliterated`
- Backend/artifact: `llama.cpp` / `gguf-q4_k`
- Admin mirror:
  `TracNetwork/mayhem-catalog-huihui-ai-Huihui-Agents-A1-abliterated-GGUF@59d0dfbbdb07138fb53fc8672cd04261efa3065e`
- Upstream pin:
  `huihui-ai/Huihui-Agents-A1-abliterated-GGUF@9189aa287362d13d803cb0d21335c0b0fd5d191c`
- Primary GGUF: 21,166,757,536 bytes. Mandatory BF16 projector:
  902,821,824 bytes.
- Canary:
  [`canary-a1-launch-v1.json`](catalog/canaries/canary-a1-launch-v1.json)

**Hard requirements and surface**

- Linux, Windows, or macOS CPU; CUDA, Metal, or Vulkan when compiled into the
  installed Mayhem build.
- 32 GiB RAM, 32 GiB full-offload target, AVX2 or NEON.
- Endpoints: OpenAI chat completions, completions, responses, and HF
  multimodal. Text, image, and video input produce text. JSON, automatic and
  required tools, and multiple tool calls in one turn are supported.
- Context ceiling: 262,144 tokens; brackets `le8k`, `le32k`, `le128k`,
  `le256k`.
- Defaults: temperature `0.85`, top-p `0.95`, top-k `20`, min-p `0`, repeat
  penalty `1`, presence penalty `1.1`.
  `thinking_mode=enabled|disabled`; no signed thinking-history or
  low/medium/high effort control is advertised.
- Signed video input is 4 through 64 frames. The larger upstream claim is not
  exposed because the canonical llama.cpp decoder proof is bounded at 64.

**Measured guidance**

- Signed image evidence uses 1 megapixel; signed video evidence uses 4 frames.
- Calibration process-tree RSS was about 19.51 GiB baseline and 24.24 GiB peak,
  an incremental 4.73 GiB. Device and OS headroom remain necessary.
- One generation is admitted in flight.

**Start**

```bash
mayhem doctor --provider-backend llama.cpp
mayhem up --provider --provider-enclave huihui-ai/Huihui-Agents-A1-abliterated --yes
```

The BF16 projector is mandatory. Do not advertise unsupported effort levels or
replace the signed projector.

## Cactus Compute Needle

**Selector and source**

- Model: `Cactus-Compute/needle`
- Canonical markets: exactly `needle-cpu` and `needle-gpu`
- Model pin:
  `Cactus-Compute/needle@5f89b4307696d669c3df1d38ae057e6e1728b107`
- Runtime source pin:
  `Cactus-Compute/needle-hf@ffd0d081401257fee31150d30c494b2f98910fc0`
- Parameter count: 30.4M

**Hard requirements and surface**

- Needle is deterministic and tools-only. It accepts 1 through 10 tools and
  does not advertise ordinary prose generation.
- The combined context ceiling is 1,024 tokens; the decoder ceiling is 512
  tokens.
- Endpoints: OpenAI chat completions and responses.
- `needle-cpu` supports Linux, Windows x86_64, and Apple Silicon macOS.
- `needle-gpu` is CUDA-only on Linux aarch64/x86_64 and Windows x86_64
  hosts and requires NVIDIA driver r580 or newer for the frozen CUDA 13
  runtime. Apple Metal/MPS is intentionally not eligible and does not create
  a third market.

**Measured guidance**

- Apple MPS decode: about 2.5 tok/s cold and 10.7 tok/s warm.
- M5 Apple CPU, one tool: 3,087 prefill tok/s and 309 decode tok/s.
- M5 Apple CPU, two parallel tools: 8,964 prefill tok/s and 329 decode tok/s.
- GB10 CUDA decode: about 89.7 tok/s cold and 166 tok/s warm.
- Windows CPU, one tool: 2,675 prefill tok/s and 126 decode tok/s.
- Windows CPU, two parallel tools: 4,933 prefill tok/s and 159 decode tok/s.
- GB10 CPU decode: about 64-78 tok/s.

Apple CPU materially outperformed MPS in the measured proof, so providers on
Apple Silicon use `needle-cpu`.

**Start**

```bash
mayhem doctor --provider-backend needle-cpu
mayhem up --provider --provider-enclave Cactus-Compute/needle --yes
```

On a supported NVIDIA host, use
`mayhem doctor --provider-backend needle-gpu` before the same managed start.
Do not map MPS to `needle-gpu` or add a third canonical market.

## Verification and troubleshooting

After startup, require all of the following rather than treating process
existence as success:

```bash
mayhem provider health --json
mayhem models --gateway
```

- `self_test.ok=true`
- every advertised `modality_health` row has `ok=true`
- at least one active serve and a fresh heartbeat
- gateway health is true and `route_count` is greater than zero for a
  compatible buyer rail
- the exact model/provider appears in the local `/v1/models` response

A failed higher-tier proof should lower the provider tier, not block Tier-1
joining. A missing canonical price bracket, inactive enclave, absent room,
artifact/signature mismatch, unsupported backend/platform pair, or failed
signed canary is a real admission failure. Report that exact reason; do not
repair it by changing a model ID, bypassing signed artifacts, creating a local
room, weakening attestation, or asking an admin to approve the provider.
