# OpenMayhem 0.2.301

- Preserve request order with bounded gateway admission queues across overlapping native provider routes. Independent routes and available provider slots can proceed concurrently.
- Cancel waiting requests promptly and expose waiting or provider selection in authenticated job status. Signed receipt and payment recovery rules remain unchanged.
- Reuse conversation affinity for ordinary chat histories without requiring custom client metadata. Briefly prefer a warm provider when recent latency measurements support waiting, within the existing admission deadline and all requested routing constraints.
- Release local admission permits after authenticated provider acceptance so receipt reconciliation cannot occupy those permits.

This release changes buyer gateway scheduling. The ledger contract remains version 30; model engines and peer transport do not require a restart for these changes. Deploy matching gateway and contained proxy-worker versions.
