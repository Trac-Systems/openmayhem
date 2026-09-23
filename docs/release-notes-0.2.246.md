# OpenMayhem Core 0.2.246

Completed durable inference jobs now remain successful when a concurrent
receipt-settlement recovery finishes after the original handoff reports a
local error. The terminal job and its exact signed receipt take precedence,
so streaming clients receive the completed result instead of a contradictory
provider failure.

Qwen3 Embedding 4B now accepts up to 128 inputs per request and advertises
up to 256 in-flight items from signed large-batch calibration evidence. Bulk
imports can use two full batches concurrently without repeating per-request
settlement overhead for every 32 inputs.

Contract version 26 and its authenticated history are unchanged.
