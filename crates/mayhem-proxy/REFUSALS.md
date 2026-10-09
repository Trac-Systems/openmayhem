# Upstream refusal evidence

This is an explicit private connector contract. It does not certify a model's identity,
change the native serving lane, settle money, authorize a retry, or infer free capacity.
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

A paid request retains the sanitized observation, but this checkpoint deliberately does
not manufacture a waiver, release customer money or issue another POST. Its retry advice
remains `recover_same_attempt`. The paid lifecycle still needs a durable refusal outcome
and explicit buyer/provider zero-charge closure integration before public serving can
be activated. Existing pre-dispatch cancellation and receipt recovery rules are unchanged.

Other upstream refusal profiles, status/cancellation recovery and already-started stream
outcomes remain required. An unknown attempt never expires merely because time passed,
a process restarted, the provider changed settings or a fresh health probe succeeded.

## Local acceptance

Real local HTTP fixtures and isolated workers cover JSON/SSE requests refused at HTTP
admission, refusal followed by successful inference, budget retention, alias scope,
conflicting/duplicate/truncated/oversized bodies, batches, explicit opt-in, stream errors,
all three paid rails and refusal replay without another POST. These checks establish the
local connector behavior; they are not real-engine or live-payment acceptance.
