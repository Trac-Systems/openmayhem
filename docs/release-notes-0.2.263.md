# OpenMayhem 0.2.263

The managed Qwen3.8 Flash-Next runtime now constrains automatic tool calls so streamed tool arguments are complete JSON before the provider returns them. Invalid tool calls remain rejected rather than executed.

OpenAI-compatible providers preserve visible answers that arrive without a separate reasoning block. Optional, bounded diagnostics record only response shape and validation results; they do not record prompts or tool arguments.

This release does not change the contract, catalog, receipt format, or other model runtimes.
