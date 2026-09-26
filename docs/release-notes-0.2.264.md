# OpenMayhem 0.2.264

OpenAI-compatible provider health checks now use bounded, spaced identity probes. A brief failure to read a runtime identity endpoint no longer retires an otherwise live provider; sustained unavailability still triggers recovery, and a mismatch with signed identity data still fails immediately. Health logs identify the failing endpoint without recording prompts or response bodies.

The provider now reports a failed health check accurately instead of claiming the runtime process exited. This release does not change the contract, catalog, receipts, or inference request format.
