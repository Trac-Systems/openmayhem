# 0.2.204

OpenAI-compatible stream adapters now derive deterministic, content-bound
pseudo-token IDs when upstream SSE omits portable token IDs. Token-fingerprint
calibration can distinguish equal-length streams by their delta content while
preserving the existing streamed text and chunk delivery.

Contract version remains 25 and this release requires no pricing migration.
