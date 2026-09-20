# OpenMayhem Core 0.2.257

- Allow cumulative compute evidence to advance across streamed receipt checkpoints while preserving monotonic validation.
- Route terminal stream-flush failures through signed failure receipts so interrupted work can settle without ambiguous reconciliation.
- Preserve the original provider stream error when an engine callback aborts, keeping request and provider failures correctly classified.
- Accept schema-12 receipts in the epoch and TAP settlement workers.

This is a wire-compatible code release. Contract version 27 and its contract digest are unchanged.
