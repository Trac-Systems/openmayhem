# OpenMayhem 0.2.216

- Admit signed managed OpenAI-compatible runtimes from their calibrated peak GPU and host-memory envelopes instead of treating downloaded artifact bytes as resident memory.
- Budget managed CUDA runtimes against detected NVIDIA memory while preserving explicit host-memory headroom.
- Keep older signed runtime bindings compatible; calibrated envelopes are enforced when present.
