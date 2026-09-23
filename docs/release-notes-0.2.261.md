# Mayhem 0.2.261

This release adds native typed decision inference through `POST /v1/decisions` and the
`convaiinnovations/laya` model family. It includes signed endpoint metadata, exact structured
canaries, deterministic request binding, asynchronous jobs, cancellation, retry and paid receipt
support across the existing payment rails.

The Laya runtime pins and preloads its English, multilingual and typed-decision checkpoints on
CUDA. Request-local routing, email preprocessing, temperatures, token limits and bounded
shortlisting are validated without mutating shared model state. Existing model classes keep their
prior contract and runtime behavior.
