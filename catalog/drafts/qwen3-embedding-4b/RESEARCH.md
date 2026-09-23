# Qwen3-Embedding-4B onboarding evidence

This tracker follows `CALIBRATION.md` v10. Source review precedes mirroring,
calibration and publication. The selected calibration provider performs
weight-bearing work; the canonical admin/indexer only applies and publishes
signed evidence.

## Pinned source

- Repository: `Qwen/Qwen3-Embedding-4B`
- Revision: `5cf2132abc99cad020ac570b19d031efec650f2b`
- License: Apache-2.0
- Gating: none
- Last source update: 2025-06-20T09:30:56Z
- Artifact: BF16 safetensors, two weight shards, 8,043,548,672 weight bytes
- Architecture: `Qwen3ForCausalLM`, hidden size 2560, 36 layers, 32 attention heads and 8 KV heads
- Popularity snapshot: 2,328,520 recent downloads and 20,559,076 all-time downloads when the pinned source metadata was captured

The immutable source manifest is in `source-manifest.json`. Raw source evidence is retained under `source/`.

Primary references:

- Model repository: https://huggingface.co/Qwen/Qwen3-Embedding-4B
- Official implementation: https://github.com/QwenLM/Qwen3-Embedding
- Pinned vLLM pooling configuration: https://docs.vllm.ai/en/v0.24.0/api/vllm/config/
- Pinned vLLM pooling API: https://docs.vllm.ai/en/v0.24.0/api/vllm/

## Proven model semantics

- Native embedding size: 2560.
- Matryoshka output dimensions: 32 through 2560 according to the model card; 1536 is required for this launch.
- Advertised context: 32K. The Transformers config contains `max_position_embeddings=40960`, while tokenizer metadata advertises a larger value. The catalog remains at the model card's 32K until the exact production runtime proves a larger supported limit.
- Pooling: last non-padding token.
- Padding: left padding in the official reference implementation.
- Normalization: L2 normalization after optional dimension truncation.
- Similarity: cosine.
- Query instruction: `Instruct: {task_description}\nQuery:{query}`. Documents use no prefix. The default source query prompt is retained in `source/config_sentence_transformers.json`.
- The service never silently adds a generic query instruction to document inputs. Callers choose the task-specific query text.

## Card-to-pipeline coverage

| Source requirement | Production implementation | Evidence required before publication | State |
| --- | --- | --- | --- |
| Qwen3 BF16 checkpoint | Pinned two-shard safetensors artifact | Mirror manifest and per-file SHA-256 verification | Source pinned |
| Last-token pooling | vLLM 0.24 pooling runner with `convert=embed` | Exact-runtime comparison against official reference vectors | Passed; minimum cosine 0.999717 |
| Left padding | Pinned tokenizer and vLLM pooling preprocessing | Token IDs and reference-vector comparison | Passed in official-reference comparison |
| L2-normalized embeddings | Model pooler/normalize configuration through vLLM | Vector norm and cosine canaries | Passed; native and 1536 norms are 1.0 |
| Matryoshka dimensions | Request native output, truncate its prefix, then L2-normalize | Native 2560 and exact 1536 vector tests | Passed; vLLM metadata does not declare MRL itself |
| String input | Existing OpenAI-compatible `/v1/embeddings` contract | HTTP/client conformance | Passed in calibration endpoint matrix |
| Array input | Concurrent per-item `AsyncLLM.encode`, stable response ordering | Batch 1, 8 and 32 conformance | Passed; order retained through batch 32 |
| 32K supported input | Catalog context and provider admission | Maximum-length and overflow tests | Passed at 32,767 caller tokens plus one internal token; 32,768 caller tokens rejected |
| Query instructions | Caller-visible documented format | Retrieval comparison with and without proper query instruction | Pending evaluation |
| Multilingual and code retrieval | Same embedding endpoint | Representative Recall@k, MRR and nDCG suite | Pending evaluation |
| Deterministic serving | Seeded exact production backend | Repeatability distribution and `embedding_cosine` tolerance | Passed; native and 1536 canaries retained |
| Prefix reuse | Mandatory vLLM prefix caching | Loaded-runtime evidence and repeat-prefix measurement | Passed; loaded configuration reports prefix caching enabled |

## Core integration decision

The public embedding endpoint, batch input schema, dimensions field, base64/float response encoding, routing, receipts and billing already exist. The vLLM engine previously exposed generation only. The generalized change adds:

- An explicit vLLM task (`generate` or `embedding`) selected from the signed model class.
- Pooling runner and embedding conversion in the pinned managed vLLM runtime.
- Concurrent batched encoding with stable input ordering.
- Dimension, count and finite-vector validation.
- Cancellation of every in-flight item in a batch.
- A parallel-safe embedding handle for independent provider sessions.
- Generation-only KV-capacity checks remain limited to generation; mandatory prefix caching remains enabled for both runners.

No Qwen model ID or model-specific route is hardcoded into Core.

## Runtime measurements

The retained `qwen3-embedding-4b-vllm-benchmark.json` uses vLLM 0.24, BF16,
32,768 model context, eight scheduler sequences, a 32,768-token scheduler
budget, and a 13% unified-memory target. It recorded:

- 57.21-second cold load.
- 32,767 caller tokens accepted as 32,768 billed/model tokens at 4,462.27
  input tokens/second; one additional caller token was rejected before work.
- Batch 1/8/32 short-input rates of 378.51, 2,358.46, and 2,432.98 input
  tokens/second.
- Four concurrent batches of eight at 5,064.32 input tokens/second and eight
  concurrent batches of eight at 5,920.53 input tokens/second.
- Sustained p50/p90/p99 latency of 38.4/39.5/41.6 ms for batch 1,
  46.2/79.4/82.1 ms for batch 8, and 169.8/205.6/206.9 ms for batch 32.
- No swap growth: 6,569,984 bytes before, during, and after the run.
- A 0.285 ms caller-visible cancellation acknowledgement. vLLM's active
  pooling kernel drained for 7.18 seconds before the recovery request finished;
  Core's independent cancellation flag discards that late vector.

The 12% vLLM target failed safely because a 32K request needs about 4.5 GiB of
KV space and only 3.41 GiB was available. Thirteen percent passed with 4.63 GiB
and 33,680 KV tokens. The launch minimum is therefore 16 GiB full-offload
memory rather than the unproven 12 GiB estimate.

## Pricing evidence and decision

Prices were checked against current first-party pages on 2026-09-16:

| Service | Current public price | Source |
| --- | ---: | --- |
| OpenAI `text-embedding-3-small` | $0.02 / 1M input tokens | https://developers.openai.com/api/docs/models/text-embedding-3-small |
| OpenAI `text-embedding-3-large` | $0.13 / 1M input tokens | https://developers.openai.com/api/docs/models/text-embedding-3-small |
| Voyage `voyage-4-lite` | $0.02 / 1M input tokens | https://docs.voyageai.com/docs/pricing |
| Voyage `voyage-4` | $0.06 / 1M input tokens | https://docs.voyageai.com/docs/pricing |
| Voyage `voyage-4-large` and `voyage-code-4` | $0.12 / 1M input tokens | https://docs.voyageai.com/docs/pricing |
| Google `gemini-embedding-2` text | $0.20 / 1M input tokens | https://ai.google.dev/gemini-api/docs/pricing |
| Cohere Embed 4 dedicated small instance | $4/hour or $2,500/month | https://cohere.com/pricing |

Measured active board power was normally 30–46 W, with an 82 W observed peak;
idle/load sampling was about 11.5 W. At EUR 0.40/kWh, 46 W costs EUR 0.0184
per active hour. A conservative shared-hardware allocation assigns 13% of a
$3,999 purchase amortized across three years, or about $0.0198/hour, to this
resident service. Combined allocated compute and power are roughly $0.04/hour
before tax and operator overhead. This is about $0.039 per million tokens at
the measured sustained single-item rate and about $0.0022 per million at the
four-request measurement. These are explicit costing assumptions, not a claim
about the owner's electricity contract or purchase price.

The reference/start price is $0.06 per million input tokens. Contract v25's
existing 25% to 400% activity band gives a lower bound of $0.015 and upper
bound of $0.24 per million. Output has no price. The existing activity
controller may move the price inside that band; provider-advertised capacity
does not affect it.

## Acceptance gates

- Passed: every file was mirrored from the pinned source with the protected
  fleet Hugging Face credential, then checked against the retained manifest.
- Passed: exact-runtime native and 1,536-dimensional vectors matched the
  official reference with minimum cosine 0.999717.
- Passed locally: four and eight concurrent batches completed with stable item
  order. Public concurrent-route proof remains pending publication.
- Passed: cold start, memory, sustained batches, cancellation, recovery, and
  maximum-length behavior are retained in the benchmark reports.
- Pending credentialed comparison: run the retained representative suite at
  1,536 dimensions against `text-embedding-3-small` and record Recall@k, MRR,
  and nDCG. Publish no match-or-beat claim unless that aggregate proves it.
- Passed: current competitor pricing and explicit operating-cost assumptions
  support the $0.06/M input-token starting reference and existing 25% to 400%
  activity band.
- Passed locally: tier-1/tier-2 endpoint matrix and `embedding_cosine` evidence.
  Signed canonical publication, readback, and paid FIAT/TNK/TAP proofs remain
  pending the synchronized release.
- Restored: the pre-existing H3 and ACE provider services returned to accepting
  state after clean calibration. Coexistence proof with the persistent embedding
  service remains pending its admitted start.

## Retained evidence hashes

- Exact-runtime benchmark SHA-256:
  `1c271e018273a46549ca9b5f35d9d26b8ffb293b4661cfa8da11744731068d4b`
- Cancellation report SHA-256:
  `928a4aeb7f1412e302b34c26d0ee30cc359a3ac260d5653a2dfc264c729099ea`
- Official-reference comparison SHA-256:
  `1eef40cec37ba00eae1db319bc93b0a33fa1b8278719fe8d98cc2c10fafac691`
- Final canary report SHA-256:
  `8d95b6f473ff29fbe13ae70a32d626f4f107a48b249b66234d36a59dca6e2e41`
