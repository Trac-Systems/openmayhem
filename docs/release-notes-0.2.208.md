# 0.2.208

The managed Qwen3.8 Flash-Next profile now selects the Triton main-attention
backend when deterministic inference is enabled. The signed launch profile
continues to use Triton linear-attention prefill, FlashInfer linear-attention
decode, and the existing radix and hierarchical cache configuration.

Contract version remains 25 and this release requires no pricing migration.
