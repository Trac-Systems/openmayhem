# Laya onboarding and calibration

Status: implementation in progress. This plan is subordinate to `docs/CALIBRATION.md` v10.

## Scope

Onboard the pinned `convaiinnovations/laya` bundle as a native typed-decision model. The bundle contains the English, multilingual, and typed-decisions checkpoints. Production providers preload all three checkpoints on CUDA and use the upstream router. SemIf/Qwen3.5-4B is explicitly out of scope.

The public surface is `POST /v1/decisions`. This is a new endpoint family and model class; Laya must not be represented as chat completion or text generation. The endpoint participates in the existing signed session, receipt, route, retry, idempotency, job, pricing, fiat, TNK, and TAP paths.

## Pinned upstream facts

- Repository: `https://huggingface.co/convaiinnovations/laya`
- Revision: `1c5edc17a7acd8701df6fc341c0d179f1c62c982`
- License: Apache-2.0
- Runtime: Python 3.10+, PyTorch, Transformers 5.x, upstream `laya.Router`
- English checkpoint: ModernBERT-large, 421M, 512-token context
- Multilingual checkpoint: mmBERT-base, 322M, 1024-token context
- Typed-decisions checkpoint: ModernBERT-large, 421M, 1024-token context
- Production mode: `Router(preload=True, device="cuda")`
- Upstream measured T4 latency: 32.8-39.5 ms for one question, 72.3-158.6 ms for ten, and 337 ms for fifty on multilingual
- Architectural limit: keep `choice` questions at 20 options or fewer
- Automatic typed-workflow detection is opt-in upstream and remains opt-in here

Research evidence is retained outside git at `.local-mayhem/laya-onboarding-20260921/research` in the parent workspace.

## Card-to-pipeline coverage

| Field | Source | Type / limit | Default | Mayhem disposition |
|---|---|---|---|---|
| `state` | README, `Agent.system_one` | string, object, or array; serialized into the selected checkpoint context | required | exposed, required |
| `questions` | README, `Agent.system_one` | object keyed by question id | required | exposed, required; bounded count and byte size |
| `questions.*.type` | README, `common.QTYPES` | `choice`, `score`, or `noul` | required | exposed, required |
| `questions.*.instructions` | README, `_to_internal` | string or JSON-serializable value | required | exposed, required and bounded |
| `questions.*.criteria` for choice | README, `render_options` | object or list; at most 20 options | required | exposed, required, max 20 |
| `questions.*.criteria` for score | README, `render_options` | ordered array | required | exposed, required and bounded |
| `questions.*.criteria` for noul | `render_options` | optional object with `false` and `true` descriptions | omitted | exposed, optional |
| `model` override | README, `Router.predict` | `english`, `multilingual`, `typed-decisions`, plus documented aliases | automatic routing | exposed as `checkpoint`; canonical values only |
| `task` | README, `Router.route` | `typed_decisions` or checkpoint alias | omitted | exposed, optional |
| `lang` | README, `Router.route` | language hint | detected | exposed, optional |
| typed workflow auto-detection | `Router(auto_task_detection=...)` | boolean | false | exposed as `auto_task_detection`, default false |
| router default | `Router(default=...)` | canonical checkpoint name | english | provider serving knob fixed to upstream default |
| preload | README, `Router(preload=True)` | boolean | false upstream generic SDK | provider serving knob fixed true to avoid 7-10 s reloads |
| device | README, `Router(..., device="cuda")` | runtime device | auto | provider serving knob fixed CUDA; CPU fallback is rejected |
| response `answers` | README, `Agent.system_one` | typed object containing probability, confidence, and action data | n/a | returned unchanged after finite-number validation |
| response `routing` | README, `Router.predict` | selected checkpoint and reason | n/a | returned unchanged after local-path/token redaction checks |
| response `usage` | `Agent.system_one` plus Mayhem's bounded response meter | exact processed input tokens plus serialized result units (one unit per four visible JSON bytes) | n/a | returned and reconciled through the standard receipt path; no per-request or minimum-session charge |

## Measured serving envelope and price

The pinned three-checkpoint bundle was loaded on two independent NVIDIA GB10
CUDA hosts with the exact frozen Python runtime. Both runs reported all three
checkpoints resident on CUDA and a peak worker allocation of 5,671 MiB. A
five-request warm probe measured 20.889 ms p95 on the first host and 20.444 ms
p95 on the second. The fuller mixed calibration probes measured 84.531 ms and
84.237 ms p95 respectively. Production load now fails closed if the device is
not CUDA or any checkpoint is absent.

The proposed Tier-1 reference rate is 10,000,000,000 au per input token and per
serialized result unit, equal to $0.01 per million units. There is no fixed
request charge and no minimum-session charge. With the platform's hard
25%-400% activity band, the full range is $0.0025-$0.04 per million units. The
floor remains a positive 2,500,000,000 au per unit, so a one-unit request is
still billable and neither fiat nor TNK/TAP accounting rounds the work to zero.
The reference price is below the upstream managed-API comparison of $0.042 per
million tokens while preserving room for every downward market step. Tier 2
uses the same calibration evidence and receives its required higher seed when
the enclaves are registered.

## Implementation order

1. Add `mayhem_decisions`, model class `decision`, endpoint contract, request validation, compatibility validation, and exact canonical JSON decision fingerprinting.
2. Add a persistent `laya` engine backend and embedded JSON-line Python worker. Load only the pinned local snapshot, preload all three checkpoints, require CUDA, reject silent CPU fallback, and preserve upstream routing behavior.
3. Add provider dispatch, signed session result handling, route retry, idempotent job storage, exact input/result-unit metering, and the gateway `/v1/decisions` response.
4. Add CLI backend lifecycle, runtime preparation, artifact verification, calibration canary support, Tier 1/Tier 2 registration, and catalog validation.
5. Download through the configured Hugging Face token, mirror/pin exact artifacts, calibrate on suitable operator hardware, and capture latency, memory, deterministic output, concurrency, and failure evidence.
6. Prove peak co-residency before selecting the two Sparks. Current candidates are Spark41 and Spark42; momentary free unified memory is not proof. Preserve at least 15-20 GiB at observed concurrent peak and do not disturb existing providers.
7. Publish the catalog row and lowest sustainable nonzero unit-rate anchor, enable the established fiat/TNK/TAP provider identities, then perform a paid micro-canary on both providers including cross-provider deterministic output and fallback.
8. Only after the canary is clean, roll the contract-bearing release across Core, gateways, helpers, workers, providers, and fleet stores under the established canonical-indexer rollout rules. Verify every process reports the same release and canonical fork before declaring completion.
9. Add the model page, API documentation, examples, and discoverability to the site after the live API is proven. Release Core and site separately.

## Acceptance gates

- English, non-English, explicit typed-decisions, all three question primitives, and invalid-shape cases pass through the paid public API.
- No request downloads or cold-loads weights.
- Both selected providers run CUDA and remain within the measured co-residency reserve under existing provider peak use.
- Automatic route, explicit checkpoint route, measured unit billing, retries, async jobs, idempotency, cancellation, Tier 1/Tier 2, and all three payment rails are verified.
- Exact canary output matches on both providers at the pinned artifact/backend fingerprint.
- No fleet-wide rollout begins before the two-provider micro-canary is complete.
