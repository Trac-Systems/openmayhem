# OpenMayhem Core 0.2.232

This release adds generic vLLM pooling support for embedding catalog models.
Providers can serve ordered batch embeddings through the existing OpenAI and
Hugging Face endpoint families with exact backend token accounting, bounded
dimensions, cancellation, and shared-worker concurrency. Signed independent
dispatch profiles now support the embedding modality while retaining the
existing text-generation rules.

The Intercom contract remains version 25 with unchanged contract bytes.
