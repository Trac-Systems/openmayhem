# OpenMayhem Core 0.2.238

Final usage receipts now remain in the durable settlement outbox until the
canonical ledger proves the exact receipt landed. A request arriving
immediately after a completed turn waits for that confirmation instead of
failing payment admission while the provider already appears available.
Streaming, non-streaming, embedding, and media routes use the same recovery
rule, and mixed provider failures preserve the most specific error category.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged.
