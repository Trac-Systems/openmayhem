# OpenMayhem Core 0.2.249

Shared vLLM embedding runtimes now size provider-session capacity from their
signed scheduler limit and operator limit. They no longer apply the decoder KV
cache reservation used for concurrent text generation. Embedding-only profiles
remain bounded by the signed batch ceiling; text, mixed-modality and isolated
worker profiles retain their existing memory admission rules.

Contract version 26 and its authenticated history are unchanged.
