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
| WanVideoWrapper MultiTalk custom nodes | `kijai/ComfyUI-WanVideoWrapper` rootless deterministic archive | `088128b224242e110d3906c6750e9a3a348a659b` | `19,021,709` | `46db1c18c57c9e025107b6569a4351fe8ae8e335d3b7d9c14b16e100e3b61d0e` | `e61b2cbd5291ae0fbf15d820e9f23876e708cb68b97ca0e48e636c3b6d1dd831` |

The WanVideoWrapper archive is a deterministic `git archive` payload with the
root directory `ComfyUI-WanVideoWrapper/` and gzip metadata disabled. The
policy must restrict the wrapper to these node classes until separately
reviewed: `Wav2VecModelLoader`, `MultiTalkModelLoader`,
`MultiTalkWav2VecEmbeds`, `MultiTalkSilentEmbeds`, and
`WanVideoImageToVideoMultiTalk`.

## Existing Signed Companions

The candidate should reuse the existing signed Wan companions rather than
adding duplicates: Wan2.1 I2V 14B 480p fp8 scaled KJ, LightX2V I2V rank64
LoRA, UMT5-XXL fp8 scaled, Wan 2.1 VAE, and Wav2Vec2 Chinese base fp16.

## Proof Gate

The candidate is not product-accepted until the new parts are mirrored into the
OpenMayhem parts dataset, added to a signed workflow policy, admitted by a real
provider, and proven through a paid `POST /v1/workflows` request. The retained
proof must show a two-character anime fight with usable dialogue/audio through
the intended OpenMayhem gateway path.
