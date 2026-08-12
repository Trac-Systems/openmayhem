# OpenMayhem Calibration

This file records the calibration gates that apply before a model or ComfyUI
workflow can be treated as product-ready. The detailed per-model facts live in
[`MODEL-CHEATSHEET.md`](MODEL-CHEATSHEET.md). The detailed Comfy workflow
parts, policies, and current class status live in
[`COMFY-CHEATSHEET.md`](COMFY-CHEATSHEET.md).

## Global Rules

- Use the intended OpenMayhem path for final proof. Direct engine or Comfy runs
  are debugging evidence only.
- Prove one representative item per model class unless a failure, backend
  difference, or contract surface makes broader coverage necessary.
- Keep contract changes exceptional. Adding a new model, tensor dtype, part, or
  workflow should use signed catalog/parts metadata whenever the current schema
  can express it.
- Use the smallest verification set that proves the changed behavior and the
  affected risk surface. Expand only from evidence.
- Do not accept local hacks, manual cache edits, out-of-policy downloads, or
  undocumented service overrides as calibration evidence.
- For multi-model hosts, start providers serially. Wait for each worker to
  verify, load, pass canary, and publish fresh heartbeats before starting the
  next worker.

## Model Calibration

A non-Comfy model is calibrated only after:

- The artifact is mirrored under the OpenMayhem Hugging Face account or another
  signed catalog source.
- The provider can install and start from the documented command without local
  overrides.
- The correct endpoint family, modalities, context boundaries, and pricing
  brackets are present in the signed catalog.
- The functional canary exercises the model class, not merely process startup.
- Native provider proof and paid gateway proof both pass for at least one
  representative route of the class.
- Measured prefill/decode or generation throughput, memory floor, backend,
  architecture, and platform limitations are recorded in
  [`MODEL-CHEATSHEET.md`](MODEL-CHEATSHEET.md).

## Comfy Workflow Calibration

A Comfy workflow is calibrated only after all gates below pass:

- Research gate: read the creator card/repo, official Comfy template, extension
  docs, and node signatures. Record controls and defaults such as prompt,
  negative prompt, dimensions, steps, sampler, scheduler, seed, guidance, LoRA
  strength, frame count, fps, audio format, voice, lipsync, upscaler choices,
  and load-plan needs.
- Inventory gate: every file the graph loads is listed in the signed
  `workflow.parts`, exists in the signed parts index, and is pulled with
  `mayhem provider parts pull`.
- Admission gate: the provider advertises parts only through
  `mayhem provider parts add`, then persists a successful
  `mayhem provider parts admit --write` envelope for the exact outcome class.
  A provider home has one Comfy inventory root; do not add disjoint workflow
  inventories into a home already serving another Comfy class unless both
  signed policies require the same root. Use separate provider homes for
  disjoint inventory roots.
- Runtime gate: every node is available in the blessed ComfyUI runtime or in a
  separately blessed extension policy. External API nodes are not local proof
  unless the catalog explicitly declares that external-service lane.
- Policy gate: the catalog row embeds or references the workflow policy,
  required parts, runtime id, node allowlist, output class, modality set,
  derivation limits, media input schema, permitted content types, user controls,
  usage unit, and reference graph hash.
- Fit gate: the selected workflow must be the right tool for the promised
  outcome. A lipsync/talking-head graph is not accepted proof for general anime
  action video with dialogue unless the paid output actually shows action,
  speech, and convincing sync.
- Input-media gate: workflows that consume user media must receive those files
  through bounded `/v1/workflows` `input_files`, not provider-local files.
- Quality gate: inspect the generated media. Correct codecs, frame counts, or
  waveforms do not pass if subject, motion, speech, sync, or image quality is
  missing.
- Paid route gate: the accepted proof must be a paid `/v1/workflows` request
  through the OpenMayhem gateway, with retained artifact and receipt/session
  evidence.

## Current Comfy Evidence

- `video.minimax_h3.t2v_i2v`: product-accepted for the base H3 T2V/I2V lane
  after signed v12 parts, admission, live provider route, and retained paid
  `/v1/workflows` anime fight proofs through the `.31` sponsored gateway. The
  current live proof `openmayhem-minimax-h3-t2v-paid-proof-v0.2.127.mp4` is
  `896x512`, `124` frames at `24` fps with AAC stereo, session
  `02e4b50f367548e156d1ca47975c1e2d213138bf4c1110311f87417af9173e58`, BLAKE3
  `f258297d464d7ee060ed9f49f38008b3880379c3cf29adb922769c8c577b7ffc`. The
  stronger dialogue-oriented proof
  `openmayhem-minimax-h3-anime-dialogue-fight-paid-v0.2.123.mp4` shows anime
  fighting action with native audio; it is still not lipsync evidence.
- `video.minimax_h3.r2v`: product-accepted for the MiniMax H3 REF2VA
  reference-media lane after signed v13 parts, `.70` admission, live route, and
  a paid fiat `/v1/workflows` proof through the `.31` sponsored gateway. The
  retained proof `openmayhem-minimax-h3-r2v-paid-proof-v0.2.126.mp4` is
  `896x512`, `124` frames at `24` fps with AAC audio, session
  `a938e5efd7ddba9a610dbf16723d7a6da62c6e6f8c9dfe924f4765c0111ba81c`, BLAKE3
  `19f92129c62ef352a2460a1a3d8654d957672fa1ae4cf0c138b3fdaea468cbfd`.
- H3 providers on GB10/Spark-class unified-memory hosts must be started
  sequentially with explicit reserve settings. The accepted `.70` T2V provider
  uses `--memory-reserve 15GB`; the default percentage reserve can falsely
  reject the same enclave by less than 1 GiB. Do not overlap base H3, R2V, and
  Spectrum reference/admission runs unless the combined resident set has fresh
  headroom evidence.
- `video.minimax_h3.spectrum`: owner-approved optional calibration target as
  of 2026-08-11. It must be handled as a distinct optional H3 enhancer
  lane: mirror and sign the Spectrum custom-node/runtime payload as a rootless
  `custom-node` `tar.gz` part with `adapter.comfy_custom_node_dir`, publish a
  bounded policy, run admission, and retain a paid `/v1/workflows` proof that
  shows audible improvement without replacing or re-proving the accepted base
  H3 and R2V lanes. This optional lane stays in the calibration backlog until
  it is proven or explicitly removed; optional does not mean exempt from the
  normal research, signed-parts, policy, admission, paid-route, and retained
  media proof gates. Current evidence: the Spectrum `custom-node` part is in
  signed parts index v14 and `.70` verified a five-part inventory root
  `0d2750e48d9b6d087233b85c0298d50323a9debc507a26cfafff99f951692864`; dry
  admission passed at `57.06GiB` required with 20% headroom, and the intended
  124-frame reference graph later passed `.70` admission with graph SHA-256
  `6a2ffd3580201483c1fa34deedb79e022235d817b7185e8615159b61d874bf71`,
  output SHA-256
  `713429ed1163b91b68116390185a3f8ec9d46f879fc95438ae59714cd8dd0887`, and
  retained artifact `openmayhem-minimax-h3-spectrum-reference-v0.2.129.mp4`.
  The signed dev catalog row now has a matching workflow-class canary proof:
  catalog hash `debf0574baf90c286e132a518986c99ddf2789890cd7b8edb7262c54694fc34a`,
  canary set `canary-minimax-h3-spectrum-workflow-launch-v1`, and endpoint
  matrix `49` cases. Paid fiat route proof passed through the `.31` sponsored
  gateway on 2026-08-12 with retained artifact
  `openmayhem-minimax-h3-spectrum-paid-v0.2.136.mp4`, session
  `c51f1fe861e46a6ebe680f1c57670f2982f81a782db7a9ce93c384ad78af7e6f`,
  BLAKE3 `25b0a156e76ad22653cee1037f6bc0fb64955e668bc5b3ecc3af39a4915ed8f2`,
  media `896x512`, `24` fps, `5.167s`, H.264/AAC stereo. Do not count it as
  product-ready public capacity until owner review confirms a measurable
  speed/quality win over the accepted base H3 lane.
- `video.heavy.le0_5mpf`: product-accepted for LTX A/V generation after paid
  `/v1/workflows` proofs with video and audio. It is not lipsync evidence.
- `image.heavy.le1_2mp`: product-accepted for Krea base image generation after
  `.42` admission and a paid fiat `/v1/workflows` proof through the `.31`
  sponsored gateway.
- `image.heavy.le17mp`: product-accepted for Krea plus signed 4x upscaling
  after `.42` admission and a paid fiat `/v1/workflows` proof through the
  `.31` sponsored gateway.
- `upscale.conv.le24mp`: signed standalone 4x convolutional upscaler policy is
  catalog-admitted with the SPANx4 part
  `d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a`.
  Local reference admission proof produced
  `openmayhem-upscale-conv-le24mp-reference-v0.2.127.png`, output SHA-256
  `3b78c0ecd45cfa63c75d2eea18c7056c417015908c7ec7c8dddc327949e4f8fc`.
  Product acceptance passed with paid fiat `/v1/workflows` proof through the
  `.31` proof gateway against the `.42` provider; retained artifact
  `openmayhem-upscale-conv-le24mp-paid-v0.2.128.png`, session
  `04a42dc2aa5159d317be3c0e80924421d1163021e91d8b41352d72ba96b940fe`,
  usage `1` `megapixel`, BLAKE3
  `1d47264316354f7f0aa787da4637711e38a0b8e78a715682cb241a937e8f9699`.
- `upscale.diffusion`: signed dev SeedVR2 diffusion upscale/restore policy is
  catalog-admitted with official Comfy-Org SeedVR2 3B int8 convrot part
  `e2a27b04c8c7244829fc5fbe3281cf7d29c7f65ef315fbb97386a66e2b3da7c7` and
  SeedVR2 VAE part
  `63e6908333939636708d0661208d534237a117d1a6a36f4c3544c1cff40be6a1`.
  `.70` reference admission passed on 2026-08-12 with inventory root
  `f501d4d7340fe2d891560aef0192adb28c0bd8e91c77b36e4ccc0e16b33fd15b`,
  graph SHA-256
  `2f8f41767f90fd18a7a7109b156ef2464f9c22648ce37ada3126e02e76fc2c93`,
  retained artifact
  `openmayhem-seedvr2-upscale-diffusion-reference-v0.2.136.png`, output
  SHA-256
  `671c24b35e99ee28edbf08b3c88c37472fbb552dfd52764f9f2d6f422149e328`,
  and canary perceptual hash `fe8181818181817f`. Paid fiat `/v1/workflows`
  proof passed through a funded gateway against the `.70` provider on
  2026-08-12 after the mainnet enclave/price/room activation. Retained artifact:
  `openmayhem-seedvr2-upscale-diffusion-paid-v0.2.138.png`, session
  `35b01e56d29b1fe11c1dd1b73fe7eca055ff35825094c7302f2a47c158bb3101`,
  usage `1` `megapixel_step`, BLAKE3
  `22895752f732ddfeb17f3aaa93fbad6b3e2bf60a9e7b9c93c7bcadd3140d1dfb`,
  output SHA-256
  `e23ce117ba5325919c642a531f69cea09dd9b36a0c27fdcb41ca14790730d9ff`,
  media `256x256` PNG.
- `video.lipsync`: technical InfiniteTalk proof exists but product quality is
  not accepted. Do not present it as the solution for anime action video with
  voice. LongCat Video Avatar 1.5 now has a dev catalog row, signed v17 parts,
  a distinct `comfyui-longcat-avatar-v0.30.1` runtime/custom-node profile, and
  `.42` reference admission using request-carried image/audio media. Retained artifact:
  `openmayhem-longcat-avatar-anime-fight-dialogue-reference-v0.2.129.mp4`,
  output SHA-256
  `d6e2622e2474c82f5324eecf4359c7877e14edbe77f811081da305460c4fc872`,
  graph SHA-256
  `7f6df53d5659f072ee4205765d6c78c7f7dc172273ca1d8ff0b19c1ad2f0431c`,
  required memory `32.12GiB`, media `832x480`, `25` fps, `100` frames, `4s`.
  Paid fiat `/v1/workflows` proof passed through a funded gateway against the
  `.42` provider on 2026-08-12 after the admission-backed startup fix. Retained
  artifact `openmayhem-longcat-paid-anime-fight-v0.2.138.mp4`, session
  `76701a2e54769c2a40b071f5902f0c8e305cfc68374766cbff516078602475b8`,
  BLAKE3 `e74704d5deec174ac2a53e70db009003986211d6ec0fbb2ac4242f77bd7137f6`,
  media `768x512`, `25` fps, `4.800s`, H.264/AAC. This closes the technical
  reference-admission, catalog-canary, and paid-route gates. It is not
  product-ready public capacity until owner review confirms intelligible speech
  and visible mouth sync; the current paid proof is mostly a two-character
  face-off/dialogue shot, not full anime choreography. The proof request had to
  use `720x480`-bounded reference input media because larger input images exceed
  the signed input-media cap and are correctly held before dispatch.

Use `scripts/verify-comfy-cheatsheet.py` whenever the signed parts index or
Comfy cheatsheet changes:

```bash
python3 scripts/verify-comfy-cheatsheet.py \
  --parts-index <parts-index-layout>/index.json \
  --outcome-grid catalog/comfy/outcome-classes-v1.json
```
