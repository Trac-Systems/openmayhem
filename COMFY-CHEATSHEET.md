# OpenMayhem Comfy Cheatsheet

This is the operational guide for ComfyUI workflow providers and users. It is intentionally tied to signed OpenMayhem policies: providers must pull signed parts, admit a bounded outcome class, and serve only graphs that fit the signed workflow envelope. User requests must never trigger provider downloads or policy bypasses.

## Current Signed Parts Index

- Dataset: `TracNetwork/openmayhem-parts-index`
- Revision: `36a1ce2720ff963f2f58555a2998d8035138932f`
- Index root: `7cd414ac0fb297bb325f8db51324ae4b58b242ed8289d160eeb1313f395f3a13`
- Anchor hash: `be3dab174f63c21b36dfded85ce9525d56e675c5dc6399237e845241deec2236`
- Index version: `13`
- Blessed runtimes: `comfyui-v0.30.1`, `comfyui-2a68ce33b4c9`
- Parts: `96` (1 audio-model, 12 checkpoint, 1 clip-vision, 35 controlnet, 7 lipsync, 2 lora, 5 text-encoder, 20 upscaler, 8 vae, 5 video-model)
- Index URL: https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/36a1ce2720ff963f2f58555a2998d8035138932f/index.json
- Anchor URL: https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/36a1ce2720ff963f2f58555a2998d8035138932f/anchor.json

## User API

Discover workflow classes and live routes from a local gateway:

```bash
curl 'http://127.0.0.1:11435/v1/models?endpoint_family=mayhem_comfy_workflows'
curl 'http://127.0.0.1:11435/v1/models?endpoint_family=mayhem_comfy_workflows&live=true'
```

Submit one paid workflow request through the gateway:

```bash
curl -X POST http://127.0.0.1:11435/v1/workflows \
  -H 'authorization: Bearer <gateway-token>' \
  -H 'content-type: application/json' \
  -d @request.json
```

Minimal request shape:

```json
{
  "model": "image.heavy.le1_2mp",
  "workflow": { "1": { "class_type": "<whitelisted-node>", "inputs": {} } },
  "response_format": "artifact"
}
```

The gateway derives required parts, graph hash, runtime id, output class, modalities, and billable usage from the signed workflow policy. Unknown nodes, unsafe paths, missing parts, output dimensions outside caps, or wrong modality sets fail before spend.

Workflow requests that consume user media use bounded `input_files` entries. Each entry must name a
safe relative filename used by the graph and carry inline base64 bytes plus content type. Providers
must not rely on files already present in their local Comfy input directory. Image inputs are
bounded PNG/JPEG; audio inputs are accepted only for formats the gateway and provider can validate
for duration and content type, currently WAV, FLAC, and MP3 when the workflow-input bridge is
present. If a workflow needs a format outside that list, add bounded validation first.

## Provider Path

Verify the local runtime and backend:

```bash
mayhem doctor --provider-backend comfyui
```

Use a real ComfyUI `v0.30.1` runtime checkout. Set `MAYHEM_COMFYUI_PYTHON` when
`python3` is not the interpreter from that runtime. On CUDA hosts, set
`MAYHEM_COMFYUI_DEVICE=cuda`; otherwise the Comfy backend intentionally defaults
to CPU and heavy canaries can take far too long.

Pull and advertise every required part for the workflow class:

```bash
mayhem provider parts pull --layout-dir <parts-index-layout> --part-id <part-id> --require-payload
mayhem provider parts add --layout-dir <parts-index-layout> --part-id <part-id>
```

Repeat both commands for every required part. Then prove the class and persist admission:

```bash
mayhem provider parts admit \
  --outcome-class <workflow-class> \
  --runtime-id comfyui-v0.30.1 \
  --part-id <part-id> \
  --part-id <part-id> \
  --usable-bytes <usable-ram-or-vram> \
  --working-set-bytes <extra-working-set> \
  --reference-graph <reference-graph.json> \
  --reference-runtime <comfy-runtime-dir> \
  --reference-output-dir <proof-output-dir> \
  --write

mayhem up --provider \
  --provider-enclave <workflow-enclave-id> \
  --artifact <comfy-runtime-dir> \
  --workflow-class-definition <definition.json> \
  --yes
```

Only use `--load-plan <plan.json>` when the signed workflow policy permits staged loading. Without a load plan, all required parts must fit together. The `--artifact` value is the local ComfyUI runtime directory; the ledger artifact remains the signed workflow class definition.

## Calibration Acceptance Gate

A Comfy calibration is not complete until all of these gates pass:

- Research gate: read the creator card/repo, official Comfy template, extension docs, and node
  signatures first. Record every user-facing control and default: prompts, negative prompts,
  dimensions, steps, sampler/scheduler, seed, guidance, LoRA strength, frame count, fps, audio
  format, voice/speaker/lipsync controls, upscaler choices, and load-plan requirements. The Mayhem
  policy is the target of the diff, not the source of truth. Also record why the selected workflow
  and parts are representative of real Comfy usage, using official templates, model-owner examples,
  or high-signal community workflows/extensions with current adoption evidence. Do not bless a
  workflow class just because one local graph happened to run.
- Inventory gate: every file the reference graph actually loads is listed in `workflow.parts`, exists in the signed parts index, and was pulled through `provider parts pull` and advertised through `provider parts add`. Manual downloads, cache leftovers, or unsourced local files are not evidence.
- Runtime gate: every node class in the graph is available in the blessed ComfyUI runtime or in a separately blessed extension policy. API-service nodes such as `sync.so` are not local OpenMayhem workflow proof unless the catalog explicitly declares that external-service policy.
- Policy gate: the catalog row embeds the workflow policy, required parts, runtime id, node allowlist, output class, modality set, derivation limits, media input file schema, permitted content types, user-facing knob ranges/defaults, usage unit, and reference graph hash. A signed outcome-class definition alone is not a routable provider market.
- Policy-fit gate: the selected workflow must be the right tool for the promised outcome, not merely a graph that runs. Record semantic fit (what task the workflow is actually good at), technical fit (quality, controllability, latency, memory, and failure modes on target hardware), and economic fit (part size, load plan, runtime cost, expected price, and whether a cheaper or better class should handle the request). A lipsync/talking-head graph is not accepted proof for general anime action video with dialogue unless it actually produces convincing action, speech, and sync through the paid path.
- Input-media gate: if the graph consumes image/audio/video files, the final proof must send them through `/v1/workflows` request media, not through provider-local files. The graph, provider admission, and paid route proof must reference the same safe filenames.
- Quality gate: inspect the generated media. A container with the right codec, frame count, or waveform does not pass if the requested subject, motion, speech, or audio quality is missing.
- Paid route gate: the final proof must go through the Mayhem provider/gateway path with `/v1/workflows`. Direct Comfy runs are useful for debugging only and must be labelled as such.
- Canary-data gate: accepted workflow canary rows must carry the complete signed-policy request shape, including every bounded `input_files` entry used by loader nodes. Do not make a placeholder graph pass by expanding the whitelist; replace placeholders with the real reference workflow and keep provider validation strict.

## Active Workflow Calibration Queue

The signed catalog currently exposes four public workflow rows plus the dev
`video.lipsync` row. Current acceptance state:

- `video.minimax_h3.t2v_i2v`: accepted for the base H3 T2V/I2V lane after
  `.70` admission and paid TNK `/v1/workflows` proofs with inspected anime
  fighting video plus native audio. The retained dialogue-oriented proof is
  `openmayhem-minimax-h3-anime-dialogue-fight-paid-v0.2.123.mp4`.
- `video.minimax_h3.r2v`: calibration in progress for the REF2VA/reference-media
  lane. REF2VA is mirrored in parts-index v13 and the workflow policy requires
  request-bound `input_files`; paid route proof and final canary fingerprint are
  still required before product acceptance.
- `video.heavy.le0_5mpf`: accepted as an LTX A/V video lane after the paid
  `.70` gateway proofs listed below. This is not lipsync evidence.
- `image.heavy.le17mp`: accepted after `.42` admission and a paid fiat
  `/v1/workflows` proof through the `.31` sponsored gateway. Retained review
  artifact: `openmayhem-krea-4x-mercedes-salespitch-paid-v0.2.123.png`.
- `image.heavy.le1_2mp`: accepted after `.42` admission and a paid fiat
  `/v1/workflows` proof through the `.31` sponsored gateway. Retained review
  artifact: `openmayhem-krea-base-mercedes-salespitch-paid-v0.2.123.png`.
- `video.lipsync`: catalog row, signed InfiniteTalk canary, and workflow-class
  modality fingerprint exist using runtime `comfyui-2a68ce33b4c9`, but the
  retained reference clip is not product-accepted: voice quality is robotic, lip
  sync is not convincing, and there is no useful background sound.

## Workflow Policy To Parts Matrix

This is the authoritative short table for which signed parts belong to which
workflow policy. A policy may exist in the dev catalog before it is
product-accepted; check the `Acceptance state` column before treating it as live
public capacity.

### Current Signed Workflow Policies

| Policy / model ID | Purpose | Required parts | Runtime | Acceptance state |
|---|---|---|---|---|
| `image.heavy.le1_2mp` | Krea 2 Turbo base image generation up to 1024x1024 | Krea 2 Turbo `6335241281bfe4537bda70cab1aca27211a9afb14197740c16778a253836bdae`; Qwen3-VL 4B text encoder `19d454e5e0516af43d0a6aee3aefd468897851bd879add036fe1b9350b66825c`; Qwen Image VAE `106d81a4897fa125d63b62fbcf2d7d1e88dc66f1b89e6f793f7142f928c7aa70` | `comfyui-v0.30.1` | Product-accepted after `.42` admission and a paid fiat `/v1/workflows` proof through the `.31` sponsored gateway; retained 1024x1024 Mercedes-sales artifact `openmayhem-krea-base-mercedes-salespitch-paid-v0.2.123.png`, session `36e05888bdeb8dce7a0fbb21a7bf4ad65efaa58e10043e1e057ab76b66524448`, BLAKE3 `a6ab8dec551c1d0e86256b50ee2a95534b2186cadb298381f8b39162650e5c1e`. |
| `image.heavy.le17mp` | Krea 2 Turbo plus 4x upscale up to 4096x4096 | Krea 2 Turbo `6335241281bfe4537bda70cab1aca27211a9afb14197740c16778a253836bdae`; Qwen3-VL 4B text encoder `19d454e5e0516af43d0a6aee3aefd468897851bd879add036fe1b9350b66825c`; Qwen Image VAE `106d81a4897fa125d63b62fbcf2d7d1e88dc66f1b89e6f793f7142f928c7aa70`; 4x-spanx4 `d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a` | `comfyui-v0.30.1` | Product-accepted after `.42` admission and a paid fiat `/v1/workflows` proof through the `.31` sponsored gateway; retained 4096x4096 Mercedes-sales artifact `openmayhem-krea-4x-mercedes-salespitch-paid-v0.2.123.png`, session `36c5476892db7056be5ddf36dcc09436ee7a44a036ed9b4b70a2877276f7e2b9`, BLAKE3 `ec80feb418f369aa509020ecad96cd6886fb8b6af6fde617a2b367a2177b872a`. |
| `video.heavy.le0_5mpf` | LTX 2.3 native audio/video generation up to 768x512, 8s, 192 frames | LTX 2.3 fp8 AV checkpoint `34dfabbf741978d452e2608769f0c83bb8b375b3b2b47185aa2b5a73430d3ae2`; Gemma 3 12B fp4 text encoder `20652c80fc8e88963343b9968722becb2118d507befbbf0272aa8d79e99893cc`; LTX distilled LoRA 384 `988522cff35f19d7c5977472be163f05b49bf381e441963da4182b0a90b1116c`; LTX spatial upscaler `e0f339c2b5c13fcae1b78cade132ae0307114026c6d20642335eccb4887a050d`; LTX audio VAE `8c108e3ce85d127cef5dbb5747f8c30d2a30c6d92f215278399224e38ffe806c` | `comfyui-v0.30.1` | Product-accepted for A/V generation after paid `.70` `/v1/workflows` proofs with retained video+audio artifacts. |
| `video.minimax_h3.t2v_i2v` | MiniMax H3 text/image-to-video with native stereo audio up to 1344x768, 15s, 362 frames | FL2VA diffusion `4c371bcbf8e7a577457d7b0ace66345fa85c88a591ca0724a5da6e9642371f72`; Qwen3VL 32B NVFP4 text encoder `32432239ffed7077993a928a915c0dc8252238657ecd4926335cfa8afff7e0ab`; H3 video VAE `3abef9354f37bb10b413e7034d373e95193511cd80ffa5aea315d1d822032ce7`; H3 audio VAE `6058c1f32eae8766393ece25f7e65871313c90197d76608b62b4ed5fac78dcd2` | `comfyui-v0.30.1` | Product-accepted for the base T2V/I2V lane: parts mirrored in parts-index v12, signed catalog metadata published, enclave/price/room live on mainnet, `.70` admission passed, and retained paid TNK `/v1/workflows` proofs passed quality review. Stronger dialogue-oriented artifact: `openmayhem-minimax-h3-anime-dialogue-fight-paid-v0.2.123.mp4`, session `286e0bf679ae7dc44c34f4979a4b53affd6c209a8ecfe774eef9380dffada1b8`, BLAKE3 `431caae7624d36a15917a1f7ee60fdd47118b35c5e025f2efc4c41aca372aae1`. |
| `video.minimax_h3.r2v` | MiniMax H3 reference-to-video / reference-media workflow | REF2VA diffusion `b5f18df20fb79f5ae577ed27d16182251712d9a1f30a29af3ffbd6526356b87b`; Qwen3VL 32B NVFP4 text encoder `32432239ffed7077993a928a915c0dc8252238657ecd4926335cfa8afff7e0ab`; H3 video VAE `3abef9354f37bb10b413e7034d373e95193511cd80ffa5aea315d1d822032ce7`; H3 audio VAE `6058c1f32eae8766393ece25f7e65871313c90197d76608b62b4ed5fac78dcd2` | `comfyui-v0.30.1` | Calibration in progress. The policy must use bounded `/v1/workflows` `input_files` for every reference image/audio/video loader and still needs paid route proof before product acceptance. |
| `video.lipsync` | Wan/InfiniteTalk lipsync/talking-video workflow up to 832x480, 4s, 81 frames | Wan2.1 I2V 14B fp8 `6a05292de329cdb06923008742e4f17329548239c2e2c3b10234276d790e1ef6`; UMT5-XXL fp8 `720ea5ea7b9de57ca87b403856b0a7e42c96d1f1176ff886726ab602b6923709`; Wan 2.1 VAE `79f0076a485bca72333bfa34c767006606b4ff351e5d8abc2045865e12c8a664`; Lightx2v I2V LoRA `6294fc7c467c664debaa9a50ea13bfd21959fe7aa29a9759f07541b66562c491`; InfiniteTalk multi fp16 `fd1d93c0ead8d77bc79d457e45bb391063a21fe3111b0a19ef7dc6a605c3b1fd`; Wav2Vec2 Chinese base fp16 `42ed9ac2d65ac013f5d5a431ff93b1e452371a6f1ba9bf8fdaa5c85b631e4f28` | `comfyui-2a68ce33b4c9` | Signed dev policy and technical canary exist; product proof failed quality review. It is not accepted for general anime action video with voice; keep it scoped to lipsync/talking-head until a better-fit policy proves action, speech, and sync. |

### Planned Or Missing Workflow Policies

| Candidate policy | Purpose | Parts needed | Missing work before serving |
|---|---|---|---|
| `video.minimax_h3.spectrum` | H3 audio-quality/smoothing extension | H3 base parts plus Spectrum H3 extension payload if accepted | Candidate quality extension. Spectrum is GPL-3.0 and still needs separate product/license acceptance before blessing. |

MiniMax H3 is owner-approved for OpenMayhem calibration as of 2026-08-10. The
five H3 payloads are mirrored and signed in the OpenMayhem Hugging Face parts
index at revision `36a1ce2720ff963f2f58555a2998d8035138932f`; base T2V/I2V
requires the first four rows, while R2V additionally requires REF2VA. The signed
mainnet catalog points to revision `4ee78a21941efe4af2fd3c8878915c39cca0f521`,
and the ledger has the base H3 enclave, price, and canonical room open. H3
became product-accepted for the base T2V/I2V lane after `.70` completed
`parts pull`/`parts add`/`parts admit --write`, served the class through
`/v1/workflows`, and retained paid TNK media proofs passed quality inspection. The stronger
dialogue-oriented proof completed through the `.31` sponsored gateway in
`327.824` seconds, returned `896x512` H.264 video, `124` frames at `24` fps,
AAC audio, usage `124` `megapixel_step`, session
`286e0bf679ae7dc44c34f4979a4b53affd6c209a8ecfe774eef9380dffada1b8`, and
artifact BLAKE3 `431caae7624d36a15917a1f7ee60fdd47118b35c5e025f2efc4c41aca372aae1`.
No local cache bypasses or unanchored Comfy payloads are acceptable for future
H3 proofs.

Use the official Comfy page and `Comfy-Org/MiniMax-H3` as sources. Prefer `int8_convrot` diffusion weights on PyTorch/CUDA 13 and
the `qwen3vl_32b_minimax_h3_nvfp4_awq` text encoder; the Comfy-Org README states that text-encoder
NVFP4 does not require Blackwell. Calibrate T2V first, then I2V/R2V only after the base lane is proven.
Required proof coverage is one clean CUDA lane, either Windows CUDA or Spark CUDA, whichever fits
fastest and safest. Add the second platform only if the first exposes platform-specific behavior.

H3 reference workflow inputs:

- `nkxx188/ComfyUI-MiniMaxH3-Easy` at commit
  `80ebae8e3847358bfb1484da3db25bf6454c3333` is a MIT custom-node/reference-workflow
  source for compact H3 T2V/I2V/first-last/reference-video graphs. Use it to understand
  working graph shape and request controls, but do not bless its prompt optimizer path:
  that path can call OpenAI/Gemini and stores API keys in plaintext JSON. Mayhem H3
  policies must keep user media, prompt, seed, dialogue, duration, and reference choices
  flexible inside the signed graph envelope, while excluding provider-side network/API
  prompt optimization.
- `xmarre/ComfyUI-Spectrum-MiniMax-H3` `v0.2.1` is an audio-quality/acceleration
  candidate for H3. Its release makes `offline_smoothing_replay=true` the default to
  avoid the reproduced single-pass speech/stutter defect, with `audio_blend_weight=0`.
  It has no declared third-party Python dependency, but it is GPL-3.0; treat it as a
  separately pinned custom-node/runtime part pending explicit license/product acceptance.

Current H3 state: official Comfy page plus `Comfy-Org/MiniMax-H3` prove native source
weights and official template availability. `ComfyUI-MiniMaxH3-Easy` is adoption evidence for a
compact H3 workflow surface, not permission to admit arbitrary optimizer/API nodes. Spectrum H3 is
candidate quality evidence for the audio path, not acceptance proof. The H3 T2V/I2V class has
signed metadata and mainnet rows, a live `.70` CUDA provider, and a retained paid TNK
`/v1/workflows` quality proof in the operator proof set. Do not replace that proof with a direct
Comfy run.

Validated H3 source manifest candidates, all from `Comfy-Org/MiniMax-H3` revision
`014cd40f7e177756c6b2473c0d93b1c89a790dd2`:

| Role | File | Part ID | Size |
|---|---|---:|---:|
| FL2VA diffusion | `diffusion_models/minimax_h3_fl2va_pruned_int8_convrot.safetensors` | `4c371bcbf8e7a577457d7b0ace66345fa85c88a591ca0724a5da6e9642371f72` | 20,970,379,616 bytes |
| Text encoder | `text_encoders/qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors` | `32432239ffed7077993a928a915c0dc8252238657ecd4926335cfa8afff7e0ab` | 15,687,142,551 bytes |
| Video VAE | `vae/minimax_h3_video_vae_fp16.safetensors` | `3abef9354f37bb10b413e7034d373e95193511cd80ffa5aea315d1d822032ce7` | 5,207,808,496 bytes |
| Audio VAE | `vae/minimax_h3_audio_vae_fp32.safetensors` | `6058c1f32eae8766393ece25f7e65871313c90197d76608b62b4ed5fac78dcd2` | 605,254,808 bytes |
| REF2VA diffusion | `diffusion_models/minimax_h3_ref2va_pruned_int8_convrot.safetensors` | `b5f18df20fb79f5ae577ed27d16182251712d9a1f30a29af3ffbd6526356b87b` | 20,970,379,616 bytes |

The initial H3 T2V/I2V policy requires FL2VA plus the shared encoder/VAEs. The
R2V policy requires REF2VA plus the same shared encoder/VAEs and must prove
reference-media behavior separately. Do not rely
on `resolve/main` or HF `ETag`; the validator must emit the immutable HF revision
above and the SHA-256 must come from Hugging Face blob/LFS metadata or a verified
payload hash. The
community `ComfyUI-MiniMaxH3-Easy` and Spectrum H3 repos are research inputs,
not permission to admit arbitrary optimizer/API nodes or GPL payloads into a
public policy.

Run this coverage check whenever the parts index or cheatsheet changes:

```bash
python3 scripts/verify-comfy-cheatsheet.py \
  --parts-index <parts-index-layout>/index.json \
  --outcome-grid catalog/comfy/outcome-classes-v1.json
```

## Outcome Classes

| Class | Title | Media | Lane | Pricing unit | Caps |
|---|---|---|---|---|---|
| `image.light.le1_2mp` | Image light <=1.2MP | image | light | megapixel_step | {"max_megapixels":"1.2","max_steps":60,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `image.light.le4_5mp` | Image light <=4.5MP | image | light | megapixel_step | {"max_megapixels":"4.5","max_steps":60,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `image.light.le17mp` | Image light <=17MP | image | light | megapixel_step | {"max_megapixels":"17","max_steps":60,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `image.heavy.le1_2mp` | Image heavy <=1.2MP | image | heavy | megapixel_step | {"max_megapixels":"1.2","max_steps":60,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `image.heavy.le4_5mp` | Image heavy <=4.5MP | image | heavy | megapixel_step | {"max_megapixels":"4.5","max_steps":60,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `image.heavy.le17mp` | Image heavy <=17MP | image | heavy | megapixel_step | {"max_megapixels":"17","max_steps":60,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `video.light.le0_5mpf` | Video light <=0.5MP/frame | video | light | megapixel_step | {"max_megapixels_per_frame":"0.5","max_seconds":30,"max_frames":720,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `video.light.le2_2mpf` | Video light <=2.2MP/frame | video | light | megapixel_step | {"max_megapixels_per_frame":"2.2","max_seconds":30,"max_frames":720,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `video.heavy.le0_5mpf` | Video heavy <=0.5MP/frame | video | heavy | megapixel_step | {"max_megapixels_per_frame":"0.5","max_seconds":30,"max_frames":720,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `video.heavy.le2_2mpf` | Video heavy <=2.2MP/frame | video | heavy | megapixel_step | {"max_megapixels_per_frame":"2.2","max_seconds":30,"max_frames":720,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `upscale.conv.le24mp` | Convolutional upscale/restore <=24MP | image | upscale-conv | megapixel | {"max_output_megapixels":"24","priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `upscale.conv.le512mp` | Convolutional upscale/restore <=512MP | image | upscale-conv | megapixel | {"max_output_megapixels":"512","priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `upscale.diffusion` | Diffusion upscale/restore | image | upscale-diffusion | megapixel_step | {"max_output_megapixels":"17","max_steps":60,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `audio.tts` | Text to speech | audio | tts | audio_second | {"max_audio_seconds":600,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `audio.generation` | Audio generation | audio | audio-generation | audio_second | {"max_audio_seconds":600,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `audio.stt` | Speech to text | audio | stt | audio_second | {"max_audio_seconds":3600,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `video.lipsync` | Lip sync | video | lipsync | frame | {"max_frames":720,"max_seconds":30,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |
| `compute.norm` | Normalized residual compute | image | residual | compute_second | {"max_compute_seconds":600,"priority_scalars":{"economy":"0.5","standard":"1","priority":"2"}} |

## Class Fit Matrix

Fitting means a part is signed and belongs to the class family. It does not make
the part automatically routable. A routable workflow also needs a signed policy,
a reference graph, a canary/proof, provider `parts add`, and provider
`parts admit --write`. The calibration graph must reference every file it loads,
and every referenced file must appear in `workflow.parts`.

| Classes | Current status | Fitting signed parts |
|---|---|---|
| `image.light.le1_2mp`, `image.light.le4_5mp`, `image.light.le17mp`, `image.heavy.le1_2mp`, `image.heavy.le4_5mp`, `image.heavy.le17mp` | Krea base and Krea+4x have class-ready policies. SD15, SDXL, Z-Image, Qwen-Image, RedCraft, and image-control workflows need their own signed policy and canary before serving. | Image checkpoints: `1d439e03` Krea 2 Turbo official, `63352412` Krea 2 Turbo fp8, `dc2b6383` RedCraft Krea 2, `95566dab` CyberRealistic SD1.5, `f8547da9` CyberRealistic XL, `e403a8dc` Illustrious XL, `ca45989f` Nova Anime XL, `9489f2e2` Nova Furry XL, `2321cb8d` PerfectDeliberate, `fdb4927c` Realism Illustrious, `49bf3097` Z-Image Turbo, `53c5bb42` CyberRealistic Z-Image. Image encoders/VAEs: `19d454e5` Qwen3-VL fp8, `e86b7075` Qwen3-VL bf16, `106d81a4` Qwen-Image VAE, `79ffae7f` FLUX/Z-Image VAE. Optional Z-Image TE helper: Z-Image Uncensored TE — Abliterated Huihui Qwen3-4B v2 (Q8 GGUF), Civitai sha `E0C5BAFC...D053F87`. Image control/support: `11a77fd6`, `16a2610c`, `180c156d`, `329dce97`, `3d8ced10`, `43b29ffc`, `5088ec20`, `5db7eae8`, `74bcdf6b`, `873a9610`, `89ca951f`, `92346602`, `8e6b6fb3`, `a213697e`, `b6e3f248`, `b90b785c`, `c3af9ca4`, `c7a89f21`, `cdd1c42c`, `d6dfa562`, `db1e1e32`, `dcf73d0e`, `dda4eb9a`, `ea14dc68`, `edb4d44e`, `f7cd56a9`. |
| `upscale.conv.le24mp`, `upscale.conv.le512mp` | Class exists. A provider policy must choose one or more signed upscaler parts and prove the exact graph. | Convolutional/restoration upscalers: `121becf8`, `17b705c5`, `23178907`, `34889283`, `522bad49`, `6a1ac0ec`, `6adc20e6`, `776268ba`, `7c3058ae`, `7c985640`, `851b706a`, `8dc290bc`, `96cfc3a4`, `a95240c0`, `b40716b2`, `c21510f4`, `d871ba30`, `dfa6e5df`. Use `e0f339c2` only inside LTX-AV latent-upscale policies. |
| `upscale.diffusion` | Class exists. SeedVR2 policies need a dedicated proof; do not mix them into a convolutional-upscale admission. | Diffusion/video restoration parts: `9c98aed7` SeedVR2 3B, `ca6bff3f` SeedVR2 7B, `63e69083` SeedVR2 VAE. Optional video preprocessing/interpolation support: `6cc88536` RIFE 4.7, `865582d1` RIFE 4.9. |
| `video.light.le0_5mpf`, `video.light.le2_2mpf`, `video.heavy.le0_5mpf`, `video.heavy.le2_2mpf` | Official LTX-AV uses `video.heavy.le0_5mpf` and has a paid mainnet proof for bounded anime fighting video with a real audio track. That proof is A/V evidence, not lipsync evidence. Other video classes need separate signed policy/canary if their caps or lane differ. | Video models: `34dfabbf` official LTX 2.3 fp8, `25055314` LTX GTAnimation low-VRAM candidate. Text encoders: `20652c80` Gemma 3 12B fp4, `720ea5ea` UMT5-XXL fp8 for Wan policies. LTX AV support: `988522cf` distilled LoRA, `e0f339c2` LTX spatial x2 latent upscaler, `8c108e3c` LTX audio VAE, `32b0af06` LTX tiny VAE. Video support: `40dd2b8b` Wan low-noise control, `660c1350` Wan high-noise control, `79f0076a` Wan VAE, `34889283` animevideo x2, `851b706a` animevideo x4, `6cc88536` RIFE 4.7, `865582d1` RIFE 4.9. |
| `video.lipsync` | Dev catalog row exists for InfiniteTalk, with runtime `comfyui-2a68ce33b4c9`, signed workflow policy, request-carried image/audio `input_files`, and workflow-class canary fingerprint. The retained reference clip is a technical graph proof only and fails quality acceptance: robotic voice, weak/absent lip sync, and no useful background sound. Public paid-route proof is still a separate gate: start a provider only after `parts pull`/`parts add`/`parts admit --write` against the same policy and then run a paid `/v1/workflows` request with acceptable speech/lipsync quality. LatentSync remains unblessed. | Signed lipsync parts: `471fb7a0` LatentSync Whisper tiny, `d4330bc7` LatentSync SyncNet, `5cebda44` LatentSync UNet, `20bbd004` MeiGen InfiniteTalk single, `d8903b87` MeiGen InfiniteTalk multi, `b36a713b` Comfy-Org InfiniteTalk single fp16, `fd1d93c0` Comfy-Org InfiniteTalk multi fp16, `6a05292d` Wan2.1 I2V 14B 480p fp8, `6294fc7c` Lightx2v I2V rank64 LoRA, `42ed9ac2` wav2vec2 Chinese base fp16. Common signed companions: `720ea5ea` UMT5-XXL fp8 and `79f0076a` Wan 2.1 VAE. |
| `audio.tts`, `audio.generation`, `audio.stt` | Classes exist but this parts index has no standalone TTS, audio-generation, or STT model policy ready for public serving. | No standalone audio-class parts are currently signed. `8c108e3c` is only the LTX AV audio VAE, and the lipsync parts are only for `video.lipsync` policies. |
| `compute.norm` | Class exists for bounded residual workflow compute, not for arbitrary unsigned execution. | No class-ready parts in this index. A workflow must publish its own signed parts and policy before admission. |

## Required Part Sets

### Krea 2 Turbo Base Image

Class: `image.heavy.le1_2mp`. Runtime: `comfyui-v0.30.1`. Output: image.

Paid proof: on 2026-08-11, `.42` served a paid fiat `/v1/workflows` request
through the `.31` sponsored gateway. Retained review artifact:
`openmayhem-krea-base-mercedes-salespitch-paid-v0.2.123.png`; session
`36e05888bdeb8dce7a0fbb21a7bf4ad65efaa58e10043e1e057ab76b66524448`;
artifact BLAKE3 `a6ab8dec551c1d0e86256b50ee2a95534b2186cadb298381f8b39162650e5c1e`;
usage `16` `megapixel_step`; response completed in `16.204` seconds.

| Selector | Part ID | Type | Purpose |
|---|---|---|---|
| `krea2_turbo_fp8_scaled.safetensors` | `6335241281bfe4537bda70cab1aca27211a9afb14197740c16778a253836bdae` | checkpoint | base generator |
| `qwen3vl_4b_fp8_scaled.safetensors` | `19d454e5e0516af43d0a6aee3aefd468897851bd879add036fe1b9350b66825c` | text-encoder | prompt encoder |
| `qwen_image_vae.safetensors` | `106d81a4897fa125d63b62fbcf2d7d1e88dc66f1b89e6f793f7142f928c7aa70` | vae | image decode |

### Krea 2 Turbo + 4x Upscale

Class: `image.heavy.le17mp`. Runtime: `comfyui-v0.30.1`. Output: image.
Required inventory root: `f58f46401fcec0a446d366daf43ce9a1318bbc4a1e00c1ace78a4a441bafe34a`.
The upscaler is part of the same workflow request and is priced/routed by the
upscaled output dimensions. The signed request timeout default/calibration is
`900000` ms because the 4x stage is part of launch canary and paid-route proof.

Paid proof: on 2026-08-11, `.42` served a paid fiat `/v1/workflows` request
through the `.31` sponsored gateway. Retained review artifact:
`openmayhem-krea-4x-mercedes-salespitch-paid-v0.2.123.png`; session
`36c5476892db7056be5ddf36dcc09436ee7a44a036ed9b4b70a2877276f7e2b9`;
artifact BLAKE3 `ec80feb418f369aa509020ecad96cd6886fb8b6af6fde617a2b367a2177b872a`;
usage `136` `megapixel_step`; response completed in `36.306` seconds.

| Selector | Part ID | Type | Purpose |
|---|---|---|---|
| `krea2_turbo_fp8_scaled.safetensors` | `6335241281bfe4537bda70cab1aca27211a9afb14197740c16778a253836bdae` | checkpoint | base generator |
| `qwen3vl_4b_fp8_scaled.safetensors` | `19d454e5e0516af43d0a6aee3aefd468897851bd879add036fe1b9350b66825c` | text-encoder | prompt encoder |
| `qwen_image_vae.safetensors` | `106d81a4897fa125d63b62fbcf2d7d1e88dc66f1b89e6f793f7142f928c7aa70` | vae | image decode |
| `4x-spanx4-ch48.safetensors` | `d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a` | upscaler | 4x upscaler stage |

### Official LTX 2.3 Audio/Video

Class: `video.heavy.le0_5mpf`. Runtime: `comfyui-v0.30.1`. Output modalities: `video,audio`. Required inventory root: `f2fae1953b9e327f264120b931a512a417910770a3bb357fae51e74c40933849`.

Paid proof: on 2026-08-10, `.70` served a paid `/v1/workflows` request for the anime fight prompt through the OpenMayhem gateway. The first retained review artifact was `openmayhem-paid-ltx-anime-fight-v0.2.118.mp4`; session `d7928582c135275086a9dccc48371b1628c9d2b884bc3a23c837463a08ccb3f5`; artifact BLAKE3 `0e218f46058a57d9f2a5799a70ee687e69f2554cee00107758b96f759ebd39be`; media `1536x832`, `24` fps, `121` frames, `5.041667s`, AAC stereo audio. A second paid dialogue-heavy anime fight review artifact was `openmayhem-paid-ltx-anime-dialogue-v0.2.118.mp4`; artifact BLAKE3 `cf92c746bdb9669857c0124295ccb9a4eb99d5f04d7c89a25d53af127c5ac2b4`; media `1536x832`, `24` fps, `121` frames, `5.041667s`, AAC stereo audio. These prove the LTX A/V lane only. Intelligible speech and lip synchronization require the separate `video.lipsync` lane.

| Selector | Part ID | Type | Purpose |
|---|---|---|---|
| `ltx-2.3-22b-dev-fp8.safetensors` | `34dfabbf741978d452e2608769f0c83bb8b375b3b2b47185aa2b5a73430d3ae2` | video-model | LTX 2.3 official fp8 audio/video checkpoint |
| `gemma_3_12B_it_fp4_mixed.safetensors` | `20652c80fc8e88963343b9968722becb2118d507befbbf0272aa8d79e99893cc` | text-encoder | official prompt encoder |
| `ltx-2.3-22b-distilled-lora-384.safetensors` | `988522cff35f19d7c5977472be163f05b49bf381e441963da4182b0a90b1116c` | lora | LTX 2.3 official distilled LoRA 384 |
| `ltx-2.3-spatial-upscaler-x2-1.1.safetensors` | `e0f339c2b5c13fcae1b78cade132ae0307114026c6d20642335eccb4887a050d` | upscaler | LTX 2.3 official spatial upscaler x2 1.1 |
| `LTX23_audio_vae_bf16.safetensors` | `8c108e3ce85d127cef5dbb5747f8c30d2a30c6d92f215278399224e38ffe806c` | vae | official audio VAE |

### Legacy LTX Low-VRAM Candidate

This set is useful for exploratory local proofing but is not the official LTX-AV lane and must not replace the five-part official AV policy.

| Selector | Part ID | Type | Purpose |
|---|---|---|---|
| `LTX 2.3 GTAnimation (fast edition)` | `25055314e4c2194d3f1655e89830c325e276fd7963be6e43448f6f477460fc54` | video-model | legacy low-VRAM path, not the official AV proof lane |
| `gemma_3_12B_it_fp4_mixed.safetensors` | `20652c80fc8e88963343b9968722becb2118d507befbbf0272aa8d79e99893cc` | text-encoder | prompt encoder |
| `LTX23_audio_vae_bf16.safetensors` | `8c108e3ce85d127cef5dbb5747f8c30d2a30c6d92f215278399224e38ffe806c` | vae | audio VAE if the lane is AV |

### Lipsync and Talking-Video Parts

These are signed support parts for lipsync/talking-video workflow classes. They are not automatically usable in arbitrary graphs; a workflow class must whitelist the relevant nodes and require the matching parts.

Current runtime status:

- InfiniteTalk: the dev calibration runtime is `comfyui-2a68ce33b4c9`. The retained reference output is `openmayhem-infinitetalk-anime-fight-louder-reference-v0.2.119.mp4` in Downloads and proves only that a bounded two-speaker anime fight graph can run with request-carried image/audio media. It does not pass quality acceptance; do not advertise it as successful lipsync.
- Input media: InfiniteTalk proofs need a source image/video frame and an audio clip carried by
  `/v1/workflows` `input_files`. WAV, FLAC, and MP3 are acceptable only when the workflow-input
  bridge validates bounded duration and writes the files into the isolated Comfy input directory
  for that request. Provider-local seed files are not evidence.
- LatentSync: the three model files are signed, but the LatentSync node pack is not part of the blessed runtime. Do not admit a LatentSync workflow until that node pack is mirrored, pinned, blessed, and covered by a canary.
- `sync.so`/HeyGen API nodes: these are remote service nodes. They are not acceptable for local OpenMayhem provider proof unless a future catalog policy explicitly declares an external-service lane and its security/payment rules.

| Selector | Part ID | Type | Purpose |
|---|---|---|---|
| `LatentSync Whisper tiny` | `471fb7a00e799bdf0fcfa907d776173319aa582a015e051e130680ee94d9817b` | lipsync | audio feature extraction |
| `LatentSync 1.6 SyncNet` | `d4330bc7e63421602af23fab2c6b3063a0c8cac983b1a21e938a3a119b2d7726` | lipsync | sync scorer/checkpoint |
| `LatentSync 1.6 UNet` | `5cebda44e4154eecfa9979a4c91a99e3838b7dac3b1e00ae1479a85c265354f5` | lipsync | generation UNet |
| `InfiniteTalk single` | `20bbd00447acdd66885255339719b1d0d8ca2ac3c1ff1445e9b258c8d4d7e099` | lipsync | Wan single-speaker lipsync |
| `InfiniteTalk multi` | `d8903b87934d344be09e38264918c082ba4c33d39aa6f85f36e0d8c2c07fc553` | lipsync | Wan multi-speaker lipsync |
| `InfiniteTalk single fp16 (Comfy-Org Wan 2.1 patch)` | `b36a713bcec2161d4385f619eae004d1bb71bab86278fd69922749539af6bad5` | lipsync | exact single-speaker patch used by the blessed ComfyUI template family |
| `InfiniteTalk multi fp16 (Comfy-Org Wan 2.1 patch)` | `fd1d93c0ead8d77bc79d457e45bb391063a21fe3111b0a19ef7dc6a605c3b1fd` | lipsync | exact multi-speaker patch used by the blessed ComfyUI template; preferred for dialogue proof |
| `Wan2.1 I2V 14B 480p fp8 scaled KJ` | `6a05292de329cdb06923008742e4f17329548239c2e2c3b10234276d790e1ef6` | video-model | exact Wan I2V base for InfiniteTalk template |
| `Wan2.1 I2V Lightx2v 480p rank64 distill LoRA` | `6294fc7c467c664debaa9a50ea13bfd21959fe7aa29a9759f07541b66562c491` | lora | exact Lightx2v speed/distill LoRA for InfiniteTalk template |
| `Wav2Vec2 Chinese base fp16 (InfiniteTalk)` | `42ed9ac2d65ac013f5d5a431ff93b1e452371a6f1ba9bf8fdaa5c85b631e4f28` | audio-model | exact audio feature extractor for InfiniteTalk template |

## All Signed Parts In This Index

Use this table to choose fitting parts for a new workflow policy. The exact record JSON is authoritative for source URLs, license evidence, adapter subdirectory, scale, and canary policy.

| Lane | Type | Name | Part ID | Size | License | Record |
|---|---|---|---|---:|---|---|
| shared | vae | Qwen-Image VAE (shared: Krea 2 + Anima + Qwen-Image) | `106d81a4...c7aa70` | 0.236 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/106d81a4897fa125d63b62fbcf2d7d1e88dc66f1b89e6f793f7142f928c7aa70.json) |
| all lanes | controlnet | BiRefNet | `11a77fd6...b83042` | 0.414 GiB | mit | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/11a77fd629eb9c3b46623b5ddfe29c3f6bad324a584a86c4d1dd4d7506b83042.json) |
| all generated output | upscaler | 4x-NomosWebPhoto-RealPLKSR | `121becf8...e77fa1` | 0.028 GiB | CC-BY-4.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/121becf87164bcedabb853d51b92c820b38d17b8f73ad5ec4fcde5530ce77fa1.json) |
| all lanes | controlnet | GroundingDINO tiny | `16a2610c...404c8b` | 0.642 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/16a2610c38bda16074cce874e24af7cc4fceeffaf1fe6a4dfb224032dc404c8b.json) |
| Illustrious/Pony/Anima (16 checkpoints) | upscaler | 4x-realesrgan-x4plus-anime-6b | `17b705c5...31d345` | 0.017 GiB | BSD-3-Clause | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/17b705c50b991c230a294a81e45334333319f9b47555247154c29b7b1831d345.json) |
| Qwen-Image | controlnet | Qwen-Image InstantX ControlNet Inpainting | `180c156d...0e4b10` | 3.94 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/180c156d1e9c67ee674e3e2119859bd19afefb0dd8c55203e213cb3f3e0e4b10.json) |
| krea2 | text-encoder | qwen3vl_4b_fp8_scaled.safetensors | `19d454e5...66825c` | 4.88 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/19d454e5e0516af43d0a6aee3aefd468897851bd879add036fe1b9350b66825c.json) |
| SDXL lanes | controlnet | IP-Adapter SDXL | `1ac6ed19...cbadbf` | 0.654 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/1ac6ed19bde9baecd26b4802477e49da273719836e5a5a52f020e4fc7bcbadbf.json) |
| krea2 | checkpoint | Krea 2 Turbo Official Comfy-Org Checkpoints | `1d439e03...40cd3a` | 12.57 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/1d439e0358bdf818c1f147ffbb729e2e706fc0991b1595f97f2cf2309b40cd3a.json) |
| ltx | text-encoder | Gemma 3 12B it fp4 mixed (LTX 2.3 text encoder) | `20652c80...9893cc` | 8.80 GiB | gemma | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/20652c80fc8e88963343b9968722becb2118d507befbbf0272aa8d79e99893cc.json) |
| Z-Image Turbo | controlnet | Z-Image Fun ControlNet Tile 2.1 lite | `20933d07...156932` | 1.88 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/20933d07dc572c7a45c95133d60063137a46b823d7700dc180770c02ee156932.json) |
| wan | lipsync | InfiniteTalk single (ComfyUI build) | `20bbd004...d7e099` | 2.53 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/20bbd00447acdd66885255339719b1d0d8ca2ac3c1ff1445e9b258c8d4d7e099.json) |
| influencer pipeline | upscaler | 4x-ArtFaces-realplksr-dysample | `23178907...4dc814` | 0.028 GiB | CC-BY-4.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/231789071bac88692b747cab60e19dda6e2cbd35ba2e8282236043ae2b4dc814.json) |
| sdxl | checkpoint | PerfectDeliberate | `2321cb8d...5c6b38` | 6.46 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/2321cb8dc6c16e7e2bbec43bba8df296b66c91ebf65bb7fada16ec9c9c5c6b38.json) |
| ltx | video-model | LTX 2.3 GTAnimation (fast edition) | `25055314...60fc54` | 16.47 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/25055314e4c2194d3f1655e89830c325e276fd7963be6e43448f6f477460fc54.json) |
| anime lanes | controlnet | LineArt sk_model2 | `329dce97...d3a3cd` | 0.016 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/329dce97d4adc92e463c9863d266a8b03b68e5a1bdee44474502981487d3a3cd.json) |
| ltx | vae | taeltx2_3 (LTX 2.3 tiny VAE) | `32b0af06...84be11` | 0.022 GiB | ltx-2-community-license-agreement | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/32b0af063555e81fd65c85b699b858fd9f3b65a72cfe284c10ab5039a984be11.json) |
| Wan 2.2 / LTX / Sulphur | upscaler | 2x-realesrganv2-animevideo-xsx2 | `34889283...0b6326` | 0.002 GiB | BSD-3-Clause | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/34889283fae69ea3a2dc515fe565251abe768b71d4dfd8c8f54b9b214b0b6326.json) |
| Wan 2.2 / LTX / Sulphur | upscaler | 2x-realesrganv2-animevideo-xsx2 | `e3e7959a...e8da6d7` | 0.002 GiB | BSD-3-Clause | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/866c3b60b4804f34c1e84dff58a600cfbd465c73/records/e3e7959a2aa0cad5eacae5673d2790c29f9239eb076543739273c6c1ce8da6d7.json) |
| ltx-av | video-model | LTX 2.3 official fp8 audio/video checkpoint | `34dfabbf...0d3ae2` | 27.14 GiB | ltx-2-community-license-agreement | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/34dfabbf741978d452e2608769f0c83bb8b375b3b2b47185aa2b5a73430d3ae2.json) |
| SDXL lanes | controlnet | IP-Adapter Plus SDXL vit-h | `3633a45c...09a8fe` | 0.789 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/3633a45ca19bd17df627362ef9b330f469082dabc53aa6ed2ca65ac8e109a8fe.json) |
| Illustrious / NoobAI / Pony | controlnet | NoobAI SDXL ControlNet canny (fp16) | `3d8ced10...bb662d` | 2.33 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/3d8ced100edf3933617400698049858206b10a31f439cae21d1bb74db4bb662d.json) |
| Wan 2.2 | controlnet | Wan2.2-Fun-A14B-Control LowNoise Q4_K_M | `40dd2b8b...7babe0` | 9.00 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/40dd2b8b47ee8fe3707412c3dc289642726d1e0eade9c7d2abb77add957babe0.json) |
| SDXL / Illustrious / Pony | controlnet | ControlNet OpenPose SDXL | `43b29ffc...900391` | 2.33 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/43b29ffc900b519e464ff71ebdfdda018b71b2a4a87b1862b1a93bc2a8900391.json) |
| shared | lipsync | LatentSync Whisper tiny | `471fb7a0...d9817b` | 0.070 GiB | openrail++ | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/471fb7a00e799bdf0fcfa907d776173319aa582a015e051e130680ee94d9817b.json) |
| z-image | checkpoint | Z-Image Turbo (tongyi) | `49bf3097...6c25d0` | 11.46 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/49bf30974fbe8db2044ac3a51cc458f52095dba030b8d01d36aed95ffd6c25d0.json) |
| all lanes | controlnet | Depth Anything V2 Small | `5088ec20...8f8131` | 0.092 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/5088ec2033ec4f3cd8d8e22b2671845a80577bc7b93da0115a4a1725bf8f8131.json) |
| photoreal lanes | upscaler | 4x-Nomos2-hq-dat2 | `522bad49...799714` | 0.130 GiB | CC-BY-4.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/522bad49d5f365a4228f203ede247ec0ed86b8efb924f049719da3437d799714.json) |
| z-image | checkpoint | CyberRealistic Z-Image Turbo | `53c5bb42...10ef16` | 6.09 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/53c5bb42154d15e9a6f1609df3258a52faf979bc16d07994347560424e10ef16.json) |
| z-image | text-encoder | Z-Image Uncensored TE — Abliterated Huihui Qwen3-4B v2 (Q8 GGUF) | `E0C5BAFC...D053F87` | 3.99 GiB | apache-2.0 | Civitai model 2193783/version 2470137; unzip and hash the inner GGUF before admission |
| shared | lipsync | LatentSync 1.6 UNet | `5cebda44...5354f5` | 4.72 GiB | openrail++ | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/5cebda44e4154eecfa9979a4c91a99e3838b7dac3b1e00ae1479a85c265354f5.json) |
| SDXL / Illustrious / Pony | controlnet | TTPlanet SDXL ControlNet Tile Realistic | `5db7eae8...090a1f` | 2.33 GiB | openrail | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/5db7eae8b1d1f22c82392579c97d50b2447ffc90caaf7d8829503aea6c090a1f.json) |
| krea2 | checkpoint | krea2_turbo_fp8_scaled.safetensors | `63352412...36bdae` | 12.24 GiB | krea-2-community-license | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/6335241281bfe4537bda70cab1aca27211a9afb14197740c16778a253836bdae.json) |
| required by both SeedVR2 builds | vae | SeedVR2 VAE fp16 | `63e69083...0be6a1` | 0.467 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/63e6908333939636708d0661208d534237a117d1a6a36f4c3544c1cff40be6a1.json) |
| Wan 2.2 | controlnet | Wan2.2-Fun-A14B-Control HighNoise Q4_K_M | `660c1350...52ede1` | 9.00 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/660c13500ebba8915f31830d3c9a9aed2610a176a17e6f55f3215e030752ede1.json) |
| photoreal lanes | upscaler | 4x_NMKD-Siax_200k | `6a1ac0ec...eee3f1` | 0.062 GiB | WTFPL | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/6a1ac0ec66970b03e07d61eb40c7408745a54ccc47f4cf244e1cc0ad6deee3f1.json) |
| all lanes | upscaler | 4x_NMKD-Superscale-SP_178000_G | `6adc20e6...cbd932` | 0.062 GiB | WTFPL | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/6adc20e68a7ad40cb29956e67bb672ced0b04eed5c757cdb578303dafacbd932.json) |
| Wan 2.2 / LTX / Sulphur | controlnet | RIFE 4.7 | `6cc88536...7310da` | 0.020 GiB | MIT | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/6cc88536fb62ad95cd352916152b2657c34bc1597a8bb40672cda961027310da.json) |
| wan | text-encoder | UMT5-XXL fp8 scaled (Wan text encoder) | `720ea5ea...923709` | 6.27 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/720ea5ea7b9de57ca87b403856b0a7e42c96d1f1176ff886726ab602b6923709.json) |
| SDXL / Illustrious / Pony | controlnet | ControlNet Union SDXL promax | `74bcdf6b...ce5365` | 2.34 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/74bcdf6b03888927644665cfc02af3aeee483e7d957048f09f3552bf4cce5365.json) |
| pre/post step, all lanes | upscaler | 1x-DeJPG-realplksr-otf | `776268ba...694181` | 0.027 GiB | CC-BY-4.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/776268ba05f5860203e7da6239441b4035bac0ff3c4864b8f86485bca9694181.json) |
| wan | vae | Wan 2.1 VAE (for Wan 2.2 A14B models) | `79f0076a...c8a664` | 0.236 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/79f0076a485bca72333bfa34c767006606b4ff351e5d8abc2045865e12c8a664.json) |
| z-image | vae | FLUX 16-channel VAE "ae.safetensors" (Z-Image lane) | `79ffae7f...73ac68` | 0.312 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/79ffae7fd76b9f05dcea88f316649b284d0808beb97eca819fcd25e4ad73ac68.json) |
| photoreal lanes | upscaler | 4x-Nomos8kHAT-L-otf | `7c3058ae...4dad8b` | 0.154 GiB | CC-BY-4.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/7c3058aefee5cf994c35da3707e0d7ce7168e15eac71436d398e39010a4dad8b.json) |
| photoreal lanes | upscaler | 4x_RealisticRescaler_100000_G | `7c985640...185e13` | 0.062 GiB | WTFPL | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/7c9856408b064228a0edc151e1deced1fc24a162b8d8aedce06f5dd7fa185e13.json) |
| Wan 2.2 / LTX / Sulphur | upscaler | 4x-realesr-animevideo-v3 | `851b706a...dfb0bf` | 0.002 GiB | BSD-3-Clause | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/851b706a764144837446357c0f1d73341cbff9fedb160c26c5d4e6ba02dfb0bf.json) |
| Wan 2.2 / LTX / Sulphur - fps doubling | controlnet | RIFE 4.9 | `865582d1...f305f3` | 0.020 GiB | MIT | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/865582d1e5218d7df071a14599554bd870204dca05242f2eba0b38436ff305f3.json) |
| SDXL lanes | controlnet | IP-Adapter Plus Face SDXL vit-h | `873a9610...100efa` | 0.789 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/873a96101cda155d49bd26c6eef0b64565958cfdafacf06c736b8cfabe100efa.json) |
| SDXL lanes | clip-vision | CLIP-Vision image encoder (ViT-H) | `89ca951f...ad7ee7` | 2.35 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/89ca951fa719a374511f8fdb5499f6ace88704cdf1423956764ccc1327ad7ee7.json) |
| ltx | vae | LTX23_audio_vae_bf16.safetensors | `8c108e3c...fe806c` | 0.340 GiB | ltx-2-community-license-agreement | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/8c108e3ce85d127cef5dbb5747f8c30d2a30c6d92f215278399224e38ffe806c.json) |
| all lanes | controlnet | 8x_NMKD-Superscale_150000_G | `8dc290bc...043b68` | 0.062 GiB | WTFPL | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/8dc290bc8b8ba3285ae61bf07e52862343755a01b78a7cab79d9fc6938043b68.json) |
| Z-Image Turbo | controlnet | Z-Image Fun ControlNet Union 2.1 lite | `8e6b6fb3...90c8cc` | 1.88 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/8e6b6fb3926020ed5b54a4aae66479626479a13b9e54131f00c5e9c2bc90c8cc.json) |
| Z-Image Turbo | controlnet | Z-Image Fun ControlNet Union 2.1 full | `92346602...83181d` | 6.25 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/92346602785bc2b71b44a0c57bfbc87e5c225191dacb9bcf4bed27b06e83181d.json) |
| sdxl | checkpoint | Nova Furry XL | `9489f2e2...e3c57c` | 6.46 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/9489f2e2aeeac3e51eace20254c3ce693a69aa69c355ea8ac33630bc3fe3c57c.json) |
| sd15 | checkpoint | CyberRealistic (SD 1.5) | `95566dab...cae879` | 3.97 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/95566dab1bbd4b97341d45e54e5f3a16ad8449a3fd2e5b7fd0853be822cae879.json) |
| pre/post step, all lanes | upscaler | 1x-DeNoise-realplksr-otf | `96cfc3a4...c2de3a` | 0.027 GiB | CC-BY-4.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/96cfc3a48479f4d964ddb43663265c06e6a454bcc3bc98484f202a079dc2de3a.json) |
| ltx-av | lora | LTX 2.3 official distilled LoRA 384 | `988522cf...b1116c` | 7.08 GiB | ltx-2-community-license-agreement | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/988522cff35f19d7c5977472be163f05b49bf381e441963da4182b0a90b1116c.json) |
| Wan 2.2 / LTX / Sulphur output | upscaler | SeedVR2 3B fp8_e4m3fn | `9c98aed7...015e78` | 3.16 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/9c98aed7d7de8a9d48ad72af7d2606ce4ae3ede88030f470a39cc02fa4015e78.json) |
| Illustrious / NoobAI / Pony | controlnet | NoobAI SDXL ControlNet lineart_anime (fp16) | `a213697e...5f03e3` | 2.33 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/a213697ec466f2613588d0c0849a0f144dfaca79cc77243b9fdda6cc375f03e3.json) |
| all lanes | controlnet | Swin2SR_RealworldSR_X4_64_BSRGAN_PSNR | `a95240c0...6b4cd6` | 0.064 GiB | Apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/a95240c005bb8344d6ec5f3e1ed3306e30f6f2fb2f244f8d063d1120366b4cd6.json) |
| all lanes | upscaler | 4x-LSDIR | `b40716b2...1d0803` | 0.062 GiB | CC-BY-4.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/b40716b28b1f71f447532b68dfaf67d3619e346f0acd5983fcd9f47a951d0803.json) |
| Illustrious / NoobAI / Pony | controlnet | NoobAI SDXL ControlNet tile (fp16) | `b6e3f248...c88e37` | 2.33 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/b6e3f2483de1ce2b06d63cafda69400d34d062ea37797c790f26a17ccbc88e37.json) |
| Qwen-Image | controlnet | Qwen-Image InstantX ControlNet Union | `b90b785c...0f1a2a` | 3.29 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/b90b785ca1a2be3085041a7930ef905e0f39624b54c4779de040f646140f1a2a.json) |
| photoreal lanes | upscaler | 4x-realesrgan-x4plus | `c21510f4...92feba` | 0.062 GiB | BSD-3-Clause | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/c21510f4c9969b34e9c865bdc9022375080a3dbf0606f58bd359edbe2992feba.json) |
| all lanes | controlnet | Florence-2 large | `c3af9ca4...506ed2` | 1.45 GiB | mit | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/c3af9ca4dd7b7b892950e4b249ca4027c101fc5d997681c51bab676c20506ed2.json) |
| all lanes | controlnet | MLSD large | `c7a89f21...efd8c3` | 0.006 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/c7a89f21b88019c11a0751d894cfb3db431910ef788952343a87e7aa1befd8c3.json) |
| sdxl | checkpoint | Nova Anime XL | `ca45989f...de231a` | 6.46 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/ca45989f9e5d5af2c0b1a3b5b3e429db5a77cd70ed3b71089db07155dede231a.json) |
| Wan 2.2 / LTX / Sulphur output | upscaler | SeedVR2 7B fp8_e4m3fn | `ca6bff3f...99cc8f` | 7.67 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/ca6bff3fd46f39d119d050da73958402f4fb530e2fd864d42fb23bcd2799cc8f.json) |
| SDXL lanes | controlnet | IP-Adapter SDXL vit-h | `cdd1c42c...6bd040` | 0.650 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/cdd1c42c52b3e8d2128b6a416b752c2dcc3178e53a136fd592b48a18c16bd040.json) |
| shared | lipsync | LatentSync 1.6 SyncNet | `d4330bc7...2d7726` | 1.50 GiB | openrail++ | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/d4330bc7e63421602af23fab2c6b3063a0c8cac983b1a21e938a3a119b2d7726.json) |
| SDXL lanes | controlnet | Fooocus LaMa (object removal) | `d6dfa562...3e09bf` | 0.095 GiB | openrail | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/d6dfa562d4f6f3e82eb80ca8dbc96b883db68e246e0d09b317bb7b06df3e09bf.json) |
| all lanes | upscaler | 4x-spanx4-ch48 | `d871ba30...965e0a` | 0.008 GiB | Apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a.json) |
| wan | lipsync | InfiniteTalk multi (ComfyUI build) | `d8903b87...7fc553` | 2.53 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/d8903b87934d344be09e38264918c082ba4c33d39aa6f85f36e0d8c2c07fc553.json) |
| all lanes | controlnet | Florence-2 base | `db1e1e32...c6ecc9` | 0.431 GiB | mit | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/db1e1e32dfa3b46913289452054af88f9ac971b04ab736c061bec324a2c6ecc9.json) |
| krea2 | checkpoint | RedCraft 赤佬3 (Krea 2) | `dc2b6383...4417f3` | 12.24 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/dc2b6383126b39fbeb0948145e31174996e2442f3498206b2e5883ff6d4417f3.json) |
| all lanes | controlnet | SAM 2.1 hiera large | `dcf73d0e...9a994a` | 0.836 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/dcf73d0e3d615c7923fd5db17d91d6585ad9529605d6f330c5ab3862de9a994a.json) |
| all lanes | controlnet | MiDaS dpt_hybrid | `dda4eb9a...f9ad27` | 0.459 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/dda4eb9a127c8ebadfc774bb900e49621a3621e90140deb9ab343c2699f9ad27.json) |
| Illustrious/Pony/Anima | controlnet | 4x_NMKD-YandereNeoXL_200k | `df5c42cf...ddbced` | 0.062 GiB | WTFPL | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/df5c42cfcd3fdefbe5a3f45cfe30bc48db4c8c86cd364bfd2b54b8ae05ddbced.json) |
| anime lanes | upscaler | 2x-ModernSpanimationV1 | `dfa6e5df...afb435` | 0.015 GiB | MIT | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/dfa6e5df632624c4c08e81e4f5bc8eb09e609087bd86ad2eaaeef90d75afb435.json) |
| ltx-av | upscaler | LTX 2.3 official spatial upscaler x2 1.1 | `e0f339c2...7a050d` | 0.927 GiB | ltx-2-community-license-agreement | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/e0f339c2b5c13fcae1b78cade132ae0307114026c6d20642335eccb4887a050d.json) |
| sdxl | checkpoint | Illustrious-XL v0.1 | `e403a8dc...a1bbe9` | 6.46 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/e403a8dca595a45a623db78af4f3058a4258ec3a5d28bb2588b643a47aa1bbe9.json) |
| krea2 | text-encoder | Qwen3-VL 4B bf16 (Krea 2 text encoder) | `e86b7075...d5200d` | 8.27 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/e86b7075b604c4897f7eee276b13a15c2ff5288c64e33168f1229e6bddd5200d.json) |
| anime lanes | controlnet | LineArt sk_model | `ea14dc68...d9457f` | 0.016 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/ea14dc684028a0699190170dc167bc5032417f51d69236b192d0a009d1d9457f.json) |
| all lanes (anime tagging, prompt extraction) | controlnet | WD14 SwinV2 Tagger v3 | `edb4d44e...67264b` | 0.365 GiB | apache-2.0 | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/edb4d44ef0a65cf4ff2c713593e83ffdf21a2503daae179a458abc16c667264b.json) |
| all lanes | controlnet | ControlNet HED | `f7cd56a9...bc905e` | 0.027 GiB | other | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/f7cd56a98f51499bee60c238d72eded40ae2b1be792beda8166dac419dbc905e.json) |
| sdxl | checkpoint | CyberRealistic XL | `f8547da9...2a85ac` | 12.92 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/f8547da9bab83d8db5e9356c6d7c1c4f583d92336ea4eea02edc8e99982a85ac.json) |
| sdxl | checkpoint | Realism Illustrious By Stable Yogi | `fdb4927c...061516` | 6.46 GiB | civitai-allow-commercial-use | [record](https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/blob/023ab52a79182d4027429c0c8a12ea5bf03b81da/records/fdb4927c4c15241d9276a1290b2aebc7441d3e9f19c8a1674f344164d6061516.json) |

## Calibration Rule

Every new Comfy calibration must declare all files the workflow actually loads, mirror missing official files into the OpenMayhem parts dataset, add them as signed parts, and require them in the workflow policy before admission. A proof that only downloads files manually or bypasses the signed parts policy is not an OpenMayhem proof.

For quality proofs, save the final paid artifacts plus a small review bundle. For images, include the output image and prompt/request JSON. For video or audio, include the media file, `ffprobe` metadata, and a contact sheet or waveform so the content can be inspected quickly before the catalog/canary is accepted.

## Publishing New Parts And Workflow Policies

Every new workflow calibration starts from the graph, not from whatever files
happen to be cached on the test machine. List every checkpoint, text encoder,
VAE, LoRA, ControlNet, upscaler, audio model, lipsync patch, and helper model
the graph loads. If a loaded file is not in the active parts index, add it to
the Comfy inventory and mirror it through the signed parts path before the
workflow policy or provider admission is allowed to pass.

Use the validator to derive canonical part IDs. Do not hand-write part IDs:

```bash
mayhem admin parts validate-yaml \
  --input docs/comfy/ImageVideoGenModelsListOpenmayhem.yaml \
  --include-drafts > validator.json
```

Mirror and verify the payload on the mirror host, not on a slow local network:

```bash
python3 scripts/comfy-mirror-from-validator.py \
  --manifest validator.json \
  --output-dir mirror-staging \
  --only-part-id <part-id> \
  --hf-token-file <hf-token-file>
```

Finalize each record with immutable license evidence and exact payload canary
hashing for weight files:

```bash
mayhem admin parts onboard \
  --input docs/comfy/ImageVideoGenModelsListOpenmayhem.yaml \
  --row-index <row> \
  --payload <verified-payload> \
  --output records/<part-id>.json \
  --min-runtime comfyui-v0.30.1 \
  --license-doc-hash <64-hex-license-doc-hash> \
  --license-ref <immutable-license-evidence-ref> \
  --license-captured-at <RFC3339-time> \
  --canary-graph-hash <64-hex-probe-graph-hash> \
  --canary-output-ref <immutable-payload-or-canary-ref> \
  --canary-tolerance-method sha256 \
  --canary-max-distance-bps 0 \
  --json
```

Upload mirrored payloads into the OpenMayhem parts dataset using the existing
`payloads/w23/sha256/<prefix>/<sha>.<ext>` layout, then add that immutable HF
mirror to each finalized record:

```bash
mayhem admin parts add-mirror \
  --record records/<part-id>.json \
  --payload <verified-payload> \
  --output records/<part-id>.json \
  --mirror-url https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/<revision>/payloads/w23/sha256/<prefix>/<sha>.<ext> \
  --mirror-kind huggingface \
  --mirror-repository TracNetwork/openmayhem-parts-index \
  --mirror-path payloads/w23/sha256/<prefix>/<sha>.<ext> \
  --mirror-revision <revision> \
  --force \
  --json
```

Build the next complete index from every existing finalized record plus the new
records. Never publish a partial replacement index:

```bash
mayhem admin parts build-index \
  --record <existing-record.json> \
  --record records/<new-part-id>.json \
  --output-dir layout-v<next> \
  --index-ver <next> \
  --blessed-runtime comfyui-v0.30.1 \
  --whitelist-ver <current-whitelist-version> \
  --outcome-classes-ver <current-outcome-classes-version>

mayhem admin parts upload-plan \
  --layout-dir layout-v<next> \
  --repo TracNetwork/openmayhem-parts-index \
  --repo-type dataset \
  --commit-message "Mayhem Comfy parts index" \
  --hf-token-file <hf-token-file>
```

The catalog workflow row must reference the new parts index/anchor, embed the
workflow policy, required part IDs, runtime id, node allowlist, output class,
modality set, caps, and reference graph hash. A class in
`catalog/comfy/outcome-classes-v1.json` alone is not a usable market.
