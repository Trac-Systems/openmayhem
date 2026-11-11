# 0.2.205

OpenAI-compatible catalog canaries now derive portable fingerprint units from
the reconstructed reasoning and visible response text. Fingerprints remain
stable when an upstream stream divides identical output into different deltas,
without changing streamed responses or usage accounting.

Catalog validation now accepts signed OpenAI-compatible KV-cache metadata and
distinguishes multimodal video input from generated video output.

Contract version remains 25 and this release requires no pricing migration.
