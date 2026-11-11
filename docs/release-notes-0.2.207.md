# 0.2.207

The managed Qwen3.8 Flash-Next profile now uses deterministic inference with
the compatible Triton linear-attention prefill backend. Its signed launch
profile continues to use FlashInfer decode and the existing radix and
hierarchical cache configuration.

Contract version remains 25 and this release requires no pricing migration.
