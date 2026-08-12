# MultiTalk Lipsync Candidate Evidence - 2026-08-12

This note anchors the next `video.lipsync` quality candidate. The current
LongCat policy is technically routeable but did not satisfy the product target
for two-character anime action with dialogue. MultiTalk is the next candidate
because it is built for multi-person conversational video and interaction
control.

## Candidate Parts

| Role | Source | Revision | Size | SHA-256 | Derived part ID |
| --- | --- | --- | ---: | --- | --- |
| MultiTalk audio projection model | `MeiGen-AI/MeiGen-MultiTalk`, `multitalk.safetensors` | `b3ccbea2f68c89fafb277b9bd907905fff7a9337` | `9,947,889,040` | `f4b48e2eb148e2407711dfc29ef411820094e5684435d5791a6d34b53fe9e1db` | `0b92e833842be82ad3369d454783895d80d25cdef9cecda1ace7cb745702c8db` |
| WanVideoWrapper MultiTalk custom nodes | `kijai/ComfyUI-WanVideoWrapper` clean rootless candidate archive | `088128b224242e110d3906c6750e9a3a348a659b` | `18,780,215` | `31ef0cb7e539bae7dbc8098816d93579bcd55782f0ad90143a0d9d06d5729758` | `07408ee8ba6e2368d827cade50ce5a3c7aad666006731010bb8c551a5a6f5719` |

The original WanVideoWrapper `git archive` payload includes tar PAX metadata
and dotfile paths. OpenMayhem accepts harmless PAX extension records, but still
rejects dotfiles and unsafe path shapes in custom-node archives. The clean
candidate archive removes the top-level folder prefix and dotfiles without
changing the reviewed source revision. The policy must restrict the wrapper to
these node classes until separately reviewed: `Wav2VecModelLoader`,
`MultiTalkModelLoader`, `MultiTalkWav2VecEmbeds`, `MultiTalkSilentEmbeds`,
`WanVideoImageToVideoMultiTalk`, and the exact Wan sampler/decode nodes needed
by the admitted graph.

## Existing Signed Companions

The candidate should reuse the existing signed Wan companions rather than
adding duplicates: Wan2.1 I2V 14B 480p fp8 scaled KJ, LightX2V I2V rank64
LoRA, UMT5-XXL fp8 scaled, Wan 2.1 VAE, and Wav2Vec2 Chinese base fp16.

## Proof Gate

The candidate is not product-accepted yet. On 2026-08-12, `.70` successfully
materialized the clean signed candidate inventory with root
`a8f570b99f081570cca7957f05bc6529f35a730490c4249a17a2a48929e6579e`, but both
the sequential-audio and parallel-audio reference graphs failed inside
`WanVideoSampler` with `AttributeError: 'NoneType' object has no attribute
'max'` from WanVideoWrapper's MultiTalk attention path. Local inspection showed
why: the wrapper tries to synthesize default two-speaker masks inside
`multitalk_loop`, but the sampler already copied `ref_target_masks = None`
before entering that loop, so no reference attention map is computed for the
two-speaker audio cross-attention. The same failure is reported upstream for
multi-audio MultiTalk/InfiniteTalk runs, so a provider-local edit is not valid
evidence.

The graph-only follow-up did work. The successful proof supplied explicit
request-carried left/right mask PNGs, combined them with the core Comfy
`ImageBatch` node, converted the batch with `ImageToMask`, and fed that single
batched `MASK` into `MultiTalkWav2VecEmbeds.ref_target_masks`. The exact
admission command used the intended `mayhem provider parts admit` path without
`--write`, the clean seven-part inventory, runtime `comfyui-v0.30.1`, and the
reference graph
`/home/trac/comfy-workflows-dev/multitalk-lipsync-proof-20260812/explicit-mask-probe-20260812T201534Z/reference-workflow-explicit-masks.json`.

Admission result:

- `ok: true`, `admitted: true`
- inventory root:
  `a8f570b99f081570cca7957f05bc6529f35a730490c4249a17a2a48929e6579e`
- peak bytes: `47743734759`
- max sessions: `1`
- retained local review artifact:
  `openmayhem-multitalk-explicit-mask-reference-v0.2.141.mp4`
- artifact media: `832x480`, `25` fps, `89` frames, `3.56s`, H.264 video with
  AAC mono audio
- artifact SHA-256:
  `376f1e3dfca3d46368df141df1d10ae282e54796a13bc249e4dde061a6db9f00`

This closes the graph/runtime blocker. It does not yet close public paid-route
acceptance because the current signed `video.lipsync` catalog row still points
at the older LongCat inventory root
`d301dcad94837f8b29471de0e7b78d0108dd08d26d273df834dfbcb3ab9ca88b`.
Advertising MultiTalk through the live gateway therefore requires a signed
catalog update that changes the `video.lipsync` policy graph, allowed nodes,
parts envelope, canary, resource evidence, and fingerprint set together. Do not
advertise this candidate until that signed catalog update is in place and a
paid `POST /v1/workflows` proof succeeds through the OpenMayhem gateway path.
