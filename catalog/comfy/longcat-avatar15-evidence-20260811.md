# LongCat Avatar 1.5 Comfy Evidence

Captured: 2026-08-11

This note anchors the first LongCat Avatar 1.5 `video.lipsync` calibration
candidate. It is not a blanket approval for arbitrary LongCat workflows,
provider-side downloads, remote API nodes, or unsigned custom-node payloads.

## Sources

- `rookiestar28/ComfyUI-LongCat-Avatar`
  - Revision: `08b4daedfaed69abaf467097f8665615b2137331`
  - License: MIT
  - Packaged payload: rootless repository tarball
  - Payload SHA-256:
    `d397c3093fd1110054a5d1eca347dd6275741c9aec28114ab7c8a2a0601e2c91`
  - LICENSE SHA-256:
    `d9236a576c2c5a3d4f0df1f5b02f006a598abf9868058ef54a7df00e9662d1fd`

- `meituan-longcat/LongCat-Video-Avatar-1.5`
  - Revision: `92016c71d5d318d0f5d84e4db30015a571484ab6`
  - License: MIT by repository metadata and README
  - First-policy payloads: official INT8 sharded base model, DMD LoRA, and
    Whisper-large-v3 audio encoder

- `meituan-longcat/LongCat-Video`
  - Revision: `03b55529b1d1d4045f5fbe14d65c8c6e8116b278`
  - License: MIT by repository metadata and README
  - First-policy payload: `vae/diffusion_pytorch_model.safetensors`

## Policy Boundary

The first admissible LongCat Avatar policy must disable runtime auto-download,
must require the signed custom node plus all model/audio selector payloads, and
must run under a distinct blessed runtime profile containing the custom node's
audio and ONNX dependencies. `Kim_Vocal_2.onnx` and vocal-separation nodes are
outside the first public policy until separately signed, admitted, and proven.

## Reference Admission

Status: reference-admitted, not yet public/product accepted.

On 2026-08-12, `.42` admitted the first LongCat `video.lipsync` reference graph
through `provider parts add` and `provider parts admit --write` using only signed
v17 inventory and request-carried image/audio media.

- Runtime: `comfyui-longcat-avatar-v0.30.1`
- Inventory root:
  `d301dcad94837f8b29471de0e7b78d0108dd08d26d273df834dfbcb3ab9ca88b`
- Graph SHA-256:
  `7f6df53d5659f072ee4205765d6c78c7f7dc172273ca1d8ff0b19c1ad2f0431c`
- Output SHA-256:
  `d6e2622e2474c82f5324eecf4359c7877e14edbe77f811081da305460c4fc872`
- Retained artifact:
  `openmayhem-longcat-avatar-anime-fight-dialogue-reference-v0.2.129.mp4`
- Media: `832x480`, `25` fps, `100` frames, `4s`, AAC mono audio
- Required memory: `32.12GiB`
- Proof host: `.42`

Review note: the retained artifact is visually stable and shows mouth changes
against the supplied dialogue audio. It proves the lipsync/talking-video
reference lane only. It does not prove full anime fight choreography, and it
does not close public acceptance until a paid `/v1/workflows` proof passes media
review.
