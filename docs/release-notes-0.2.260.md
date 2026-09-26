# OpenMayhem 0.2.260

Gateway admission now refreshes authenticated catalog prices after an explicit pre-execution price-version refusal. Streaming chat, non-streaming chat, embeddings, image generation, speech, transcription, and artifact/workflow requests can retry with current terms without penalizing a healthy provider.

Retries preserve caller price limits, provider filters, cancellation, retry budgets, and prior signed usage. Concurrent refusals share catalog refresh work. Previously billed usage retains its original charge when a continuation uses a newer price.

Post-session metering and health probes retain the catalog snapshot accepted for the completed session. TNK and TAP deposit launchers now honor the configured installed binary and fail clearly if it is missing.

This release does not change the contract, catalog calibration requirements, receipt schema, or stored ledger data.
