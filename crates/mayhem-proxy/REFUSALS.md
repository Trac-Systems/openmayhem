# Upstream refusal evidence

This is an explicit private connector contract. It does not certify a model's identity,
change the native serving lane, settle money or authorize a retry. Verified negative
execution evidence can release the owning request's capacity; it does not certify
that the upstream is healthy or has spare capacity for other requests.
Default `http_status` and `open_ai` connections retain their conservative execution
uncertainty. No production connection is automatically switched to this profile.

## vLLM admission v1

Set `error_profile: "vllm_admission_v1"` only for a direct vLLM endpoint conforming to the
contract below. The selection is part of the protected connection fingerprint/revision;
it cannot be set by a public buyer request, upstream message, or model name. A relay that
changes these semantics needs its own qualified profile. This is not a blanket assertion
about every vLLM release.

Research pinned to upstream commit `dd30786fef8b15bd83205833c7b3348b0ba0ebc7`:

- [Admission checks](https://github.com/vllm-project/vllm/blob/dd30786fef8b15bd83205833c7b3348b0ba0ebc7/vllm/v1/engine/async_llm.py): queue and pending-prefill checks reject before adding the request to the engine. Multi-prompt API requests can make more than one engine invocation.
- [Exception definitions](https://github.com/vllm-project/vllm/blob/dd30786fef8b15bd83205833c7b3348b0ba0ebc7/vllm/exceptions.py): `QueueOverflowError` and `MaxQueuedTokensError` carry HTTP 503 and distinct fixed messages.
- [Error serialization](https://github.com/vllm-project/vllm/blob/dd30786fef8b15bd83205833c7b3348b0ba0ebc7/vllm/entrypoints/serve/exception_handling/error_response.py): these admission errors serialize differently from generation failures. A generic 503 or a generation-error type is insufficient.

The broker requires all of:

1. Chat Completions or a single Completions prompt, one sample, no beam search. String
   prompts and one token vector are eligible; batches, multiple samples, Responses and
   Decisions are not. Request inspection does not retain copies of prompt/tool/message data.
2. HTTP 503 with a complete `application/json` body no larger than 16 KiB, obtained within
   the existing two-second diagnostic-read bound. This bound never cuts off generation.
3. Exactly the documented error envelope: matching integer code, type, null/missing param,
   and one of the two fixed admission messages. Duplicate fields, unknown fields, extra
   job/result fields, truncation, conflicting values and other messages are rejected as
   evidence. No substring matching or vendor-message logging.

It produces a sanitized `execution: rejected` observation with a closed-vocabulary reason.
Its scope is the engine connection, since that queue includes aliases of the same engine.
The exact request/connection binding is retained by the owning paid attempt or probe.
A running stream error does not qualify: SSE/JSON error events after HTTP 200 remain
unknown execution. The isolated decoder cannot mint an admission-refusal claim.

## Consequences and current limits

An operator probe with this verified HTTP refusal releases its durable allocation using a
bound completion commitment, keeps its consumed attempt/cost allowance and marks the
connection Busy. It does not mark Ready. Recovery requires backoff plus a new explicitly
budgeted probe or suitable fresh organic evidence. Valid success releases the probe and
publishes the observed readiness; it still cannot invent native-token speed evidence.

A paid request atomically retains the sanitized failure and a `NotExecuted` outcome
bound to its original invocation, attempt, request, connection, accepted terms and rail.
Any retained result, upstream job ID or possible output delivery contradicts that claim
and prevents this transition. Strictly recognized parent failures before dispatch use
the same path. Generic HTTP errors and ambiguous execution never qualify.

The paid executor releases that request's physical allocation, independently of financial
acknowledgment. Managed exchange offers a provider-signed zero-charge waiver and replays
the same offer on `Status`. The buyer verifies its original request, accepted terms,
provider signature and evidence commitment, checks for contradictory received output or
canonical receipts, then explicitly approves and countersigns. Only canonical confirmation
of that existing mutually signed closure releases the financial hold. This is a signed
provider assertion, not independent buyer proof of upstream behavior or permission to POST
again. No new ledger operation is introduced. Retry advice stays `recover_same_attempt`.

Missing acknowledgment, interrupted publication and restart retain the exact original
intent. An older saved verified failure is reconciled one owned record at a time when
accessed; there is no history scan or blanket migration. Old cancellation/terminal waiver
drafts without the new optional evidence field preserve their original semantics and
signatures. Unknown attempts, raw errors and partial streams do not gain this shortcut.

Other upstream refusal profiles, status/cancellation recovery and already-started stream
outcomes remain required. An unknown attempt never expires merely because time passed,
a process restarted, the provider changed settings or a fresh health probe succeeded.

## Local acceptance

Real local HTTP fixtures and isolated workers cover JSON/SSE requests refused at HTTP
admission, refusal followed by successful inference, budget retention, alias scope,
conflicting/duplicate/truncated/oversized bodies, batches, explicit opt-in, stream errors,
all three paid rails, signed zero-charge closure, pending acknowledgment/restart recovery,
contradictory evidence, binding/signature tampering and refusal replay without another POST.
These checks establish local behavior; they are not real-engine or live-payment acceptance.
