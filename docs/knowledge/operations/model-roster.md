# Model roster decisions

The signed catalog is the source of truth for the current model and workflow
roster. This document records durable operating decisions that should not be
inferred from a temporary provider state.

## Extensible model support

Existing model classes and workflows remain in Core for compatibility. Future
models, workflows, and model types should move toward signed, versioned add-ons
that declare their endpoint contract, artifacts, runtime adapter, canaries,
resource limits, and billing dimensions without requiring every participant to
install a new Core contract. Core should retain only the stable verifier,
sandbox, settlement, and add-on lifecycle. A new add-on must fail closed on an
older runtime and must not interrupt unrelated providers during publication or
rollback.

Qwen3 Embedding 4B is intentionally calibrated through the existing monolithic
path for this release. Its generic vLLM pooling support belongs in Core because
the current runtime could not execute embedding catalog models. Its model
identity, artifact, endpoint limits, dimensions, canaries, price, and platform
proof remain signed catalog data.

## Qwen3 Embedding 4B

- Canonical model: `Qwen/Qwen3-Embedding-4B`
- Exact source revision: `5cf2132abc99cad020ac570b19d031efec650f2b`
- Exact mirror revision: `909825755dc39379f3eb31256602da9cafb29c95`
- Calibrated backend: vLLM 0.24 pooling, BF16, Linux NVIDIA compute capability
  12.1
- Context: 32,768 model tokens
- Dimensions: native 2,560 and Matryoshka 32 through 2,560, including exact
  1,536
- Batch input: ordered arrays up to 32 items
- Settlement rails: FIAT, TNK, and TAP with same-currency settlement
- Windows support is unavailable until a separate Windows CUDA proof exists.

Weight-bearing calibration runs on the selected provider host. Catalog
application, signing, and publication run on the canonical admin/indexer and do
not load model weights. A Core change requires one identical release across all
providers, gateways, helpers, relays, and dependent workers. Every store must
remain on the indexer's canonical fork; stores are never deleted to repair a
rollout.
