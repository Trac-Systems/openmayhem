# Durable Text Streaming

The SC-Bridge live chat producer serves `stream: true` on all three text routes:

| Route | Endpoint family | Stream format |
| --- | --- | --- |
| `/v1/chat/completions` | `openai_chat_completions` | Chat chunks, usage when requested, `[DONE]` |
| `/v1/completions` | `openai_completions` | `text_completion` chunks, internal usage, `[DONE]` |
| `/v1/responses` | `openai_responses` | Named `response.*` events; no `[DONE]` |

Chat's existing `hf_multimodal_chat` family also uses durable streaming jobs.
Development shims remain synthetic and unbillable. Signed endpoint validation,
normalized requests, provider routing, and receipt verification remain in force.
The completion/Responses adapters request internal usage without adding that
transport option to their normalized signed endpoint requests.

`GET /mayhem/status` exposes `durable_streaming_jobs: true` both at the root and
under `capabilities`, plus `durable_streaming_endpoint_families` at the root.

## Correlation and Recovery

Persist the original request key before dispatch. Submit it in `Idempotency-Key`
and retain `x-mayhem-job-id` from the response. No session header is emitted:
failover can change sessions while the job remains stable. `Prefer: respond-async`
does not suppress a new request's live stream.

If headers are lost, use the same bearer token and key with:

```http
GET /v1/jobs/lookup?endpoint_family=openai_chat_completions
Idempotency-Key: original-request-key
Authorization: Bearer original-token
```

Lookup validates the family and key, derives the owner-scoped ID, and reads one
vault entry without scanning, purging, reconciling, or dispatching inference.
Unknown/inaccessible jobs return 404. Active or reconciliation-pending jobs return
202; terminal jobs return 200. Responses include the job header and metadata.
There are no caller-supplied session or billing identifiers.

Resubmitting an identical request/key returns existing job JSON, never another
stream or inference: 202 while active/reconciling, 200 completed, 409 failed or
cancelled. Changed requests using the same key return 409. Keys are scoped by
owner token and endpoint family.

## Signed Evidence

Production receipt-bearing jobs retain the following independently of rotating
receipt history:

```text
receipt: {
  body, enclave_sig, enclave_pubkey,
  receipt_ack: { session_id, seq, user_sig },
  reconciliation: {
    transport_peer, open_timeout_millis, terminal_status,
    terminal_error, ack_reason, settlement_feature?
  }
}
```

The serialized finality field is `receipt.body.final`, not `final_receipt`.
Checkpoint evidence has `final: false`. The raw body and signatures are retained,
not merely `receipt_summary()`. Final evidence is staged before ACK/settlement;
successful terminal stream events follow durable job completion. ACK failures
emit stream errors and preserve `reconciliation_pending` evidence for recovery.

Reservations are persisted before dispatch when a durable job directory is
configured. After restart, a reservation without receipt evidence is a failed job
with `error_info.code: gateway_execution_interrupted`, `retryable: false`, and an
unknown billing outcome. Missing evidence does not establish zero cost.
Verified checkpoints reopen as reconciliation-pending jobs and reconcile without
inference. Active records cannot be evicted; pending reconciliation is exempt from
TTL eviction. Terminal records retain the vault's existing TTL/count/byte limits,
so clients must reconcile promptly and must not redispatch expired request keys.

The website should drain Core's stream after its customer disconnects. If Core's
own HTTP consumer disconnects, existing cancellation/partial-receipt handling
applies; lookup remains the recovery mechanism, not SSE replay.

Responses emits response/item/content lifecycle events, text/function-argument
deltas and done events, then `response.completed`, `response.incomplete`, or
`response.failed`. Every event has a sequence number and stable item identities.
Successful terminal `response.mayhem` carries the internal usage receipt metadata.

## Failed generation and reservation closure

A provider error may carry a signed final receipt for exactly the last
acknowledged usage and attribution. It does not add a cancellation quantum.
The gateway durably stages that receipt and its original non-retryable error
before sending the buyer ACK. A lost settlement handoff remains recoverable,
and successful accounting leaves the job `failed`.

Before accepted compute starts, the provider journals the reservation binding
under `receipt-settlement/provider/reservation-recovery`. An OS lock prevents
closure while the accepted session still owns it. When the session ends or the
process exits, background recovery delivers queued receipts and submits the
existing provider-authorized reservation close. Submissions persist before
sending and are removed only after exact confirmed closure. Delayed canonical
head changes rebuild stale submissions. Journal admission is bounded.

The gateway also persists the accepted signed voucher, including the logical
billing baseline on failover. Recovery checks exact confirmed reservation,
close record and canonical receipt head bindings and signatures. A partial
receipt remains `final: false`; `canonical_settlement` carries the separate
proof that it is now terminal. An absent head plus confirmed zero-retention
closure proves this attempt spent zero. Missing state or a network timeout
does not. If the provider never returns, buyer-authorized expiry uses the
canonical expiry epoch plus receipt grace, never wall-clock age.

Recovery continues after an initially empty queue and rotates bounded batches.
The website stores closure proof before settling confirmed cumulative usage
or releasing zero-spend holds. It preserves failed execution status and the
specific error; ledger operations remain idempotent. Normal final receipts
and existing reservation-close contract operations are unchanged. Older
provider attempts need their ended-session binding restored to the recovery
journal, or canonical expiry, before they can automatically close.
