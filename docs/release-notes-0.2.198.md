# 0.2.198

Adds signed, managed OpenAI-compatible generation runtimes. Catalog artifacts can
now bind an exact model snapshot, runtime recipe, platform wheel, container image,
hardware envelope, concurrency limit, and capability preflight. Core verifies those
bindings before a provider advertises the route and owns the runtime through cleanup
or recovery.

Streaming, reasoning, tools, structured output, prefix caching, cancellation, and
concurrent generation are admitted only when the signed runtime proves the claimed
behavior. Existing engines and catalog entries retain their prior behavior. Contract
version remains 25 and this release requires no pricing migration.
