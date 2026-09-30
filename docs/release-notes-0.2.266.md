# OpenMayhem 0.2.266

- Support separately signed managed runtime profiles for the same model artifact, preserving its existing market identity and routing filters across supported architectures.
- Seal directory model artifacts as bounded streams instead of retaining checkpoint plaintext in memory during first-time provider startup.
- Resolve additional runtime files from the selected signed execution profile without changing the canonical enclave identity.
- Run managed runtime identity probes outside the provider dispatch loop. A scheduler-dependent status response delayed during an active generation no longer retires an otherwise responsive provider; explicit identity mismatches and sustained frontend failures still do.
- Validate managed runtime context, capacity, capabilities and memory requirements against the selected signed profile.
- Bind precision-preserving recurrent-state checkpoint kernels into the managed GB10 runtime recipe while retaining prefix caching and speculative decoding.
- Request the required Stripe Connect capabilities for Canadian providers, extending the US onboarding correction.
- Allow read-only recovery lookup for all signed gateway job families.
- Refresh expired TNK settlement drafts that have not been submitted.

Contract version 28 and its code digest are unchanged. Existing model artifacts and baseline runtime profiles remain unchanged.
