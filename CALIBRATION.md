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
- `video.minimax_h3.spectrum`: owner-approved calibration target as of
  2026-08-11. It must be handled as a distinct H3 audio-quality workflow lane:
  mirror and sign the Spectrum custom-node/runtime payload, publish a bounded
  policy, run admission, and retain a paid `/v1/workflows` proof that shows
  audible improvement without replacing the accepted base H3 and R2V lanes.
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
  A paid route proof is still required before calling live marketplace capacity
  product-accepted.
- `video.lipsync`: technical InfiniteTalk proof exists but product quality is
  not accepted. Do not present it as the solution for anime action video with
  voice.

Use `scripts/verify-comfy-cheatsheet.py` whenever the signed parts index or
Comfy cheatsheet changes:

```bash
python3 scripts/verify-comfy-cheatsheet.py \
  --parts-index <parts-index-layout>/index.json \
  --outcome-grid catalog/comfy/outcome-classes-v1.json
```
