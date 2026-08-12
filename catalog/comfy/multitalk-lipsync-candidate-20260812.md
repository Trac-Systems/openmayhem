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

The candidate is not product-accepted. On 2026-08-12, `.70` successfully
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

The next admissible proof path is graph-only if it works: supply explicit
request-carried left/right mask PNGs, combine them with the core Comfy
`ImageBatch` node, convert the batch with `ImageToMask`, and feed that single
batched `MASK` into `MultiTalkWav2VecEmbeds.ref_target_masks`. This uses only
bounded `/v1/workflows` input media and reviewed node classes. If that graph
still fails, the clean follow-up is a new signed WanVideoWrapper custom-node
part that fixes mask propagation, not a local runtime patch.

Do not advertise this candidate until the exact admitted graph produces
retained quality media through a paid `POST /v1/workflows` proof. The retained
proof must show a two-character anime fight with usable dialogue/audio through
the OpenMayhem gateway path.
