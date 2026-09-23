# OpenMayhem 0.2.217

- ComfyUI providers can apply a calibrated GPU-memory reserve with
  `MAYHEM_COMFYUI_RESERVE_VRAM_GB`, allowing the runtime to offload more model
  state while preserving capacity for another local workload.
- Invalid, non-finite, negative, or unreasonably large reserve values fail
  provider startup instead of reaching the ComfyUI runtime.
