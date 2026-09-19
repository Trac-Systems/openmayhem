# OpenMayhem Core 0.2.236

This release preserves the embedding provider's signed tokenizer usage through
response collection and validates any streamed usage against it. Embedding
receipts now settle when the model tokenizer count legitimately differs from
the gateway's routing estimate, while mismatched, incoherent, or out-of-bounds
usage remains rejected.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged.
