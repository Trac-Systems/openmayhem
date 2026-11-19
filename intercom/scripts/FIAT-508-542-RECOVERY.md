# FIAT 508/542 local recovery

The v27 plan for 508 and the v28 plan for 542 start the same provider revision at
8,028,518,251,457,218,441 paid AU. The older output reserves €0.89. The frozen
542 output at index 1 reserves €1.64 and covers the older interval. Index 0 of
542 has a separate €6.78 Stripe payment. Index 2 already retains the operator
fee. Epoch 508 still has its own unresolved fee output.

Use the offline helper from the Core repository root. It reads local files and
writes a new private file for each result. Its prepared features use the ordinary
canonical admin signature and writer admission checks.

```sh
node intercom/scripts/fiat-preparation-recovery.mjs inspect SNAPSHOT NEW_INSPECTION_FILE
node intercom/scripts/fiat-preparation-recovery.mjs prepare SNAPSHOT REVIEW NEW_FEATURE_FILE KEYPAIR
```

A snapshot contains `signed_length` and `rows`. Each row is an exact confirmed
state response with `key`, `value`, `confirmed: true` and that same signed length.
Include explicit null rows for absent keys. The helper reports a missing row
when the export is incomplete. Preparation also reads the applicable canonical
bindings, holdback parameters, provider and dispute state during its in-memory
preflight. Use a disposable administrator and wallet in an isolated rehearsal.
The wallet helper accepts its password JSON on stdin. Keep the inspection,
evidence packet and signed features private.

1. Inspect the original snapshot. Copy `admin`, `old_operation`,
   `new_operation`, `state_hash` and `signed_length` into the review. Set
   `stage` to `fence`, `at` to the observed Unix time and `attestation` to
   `null`. Retain the signed fence file. Applying it in the isolated ledger
   fences the exact old attempt and preserves its request, original status and
   idempotency lock.
2. Inspect a later confirmed snapshot that includes the fence. Drain every old
   sender, queued retry and in-flight Stripe request before collecting effect
   evidence. Account for the complete destination transfer history from before
   508 through the post-drain observation. Match transfers by account,
   destination, operation, attempt, revision, watermark, FX terms and amount.
   Include both overlapping operations and identify 542's paid siblings.
   A transfer-group search, a local waiver or an expired quote alone leaves the
   recovery pending.
3. Create a **new review** using the five identity fields from the later
   inspection and a new `at`. For `supersede_absent`, include an `attestation`
   object with these fields.

   | Field | Reviewed value |
   | --- | --- |
   | `fence_key` | Confirmed fence feature key from the inspection |
   | `sends_drained_at` | Unix time all old senders and in-flight requests were drained |
   | `history_from`, `history_through` | Inclusive Unix-time bounds of the fully inspected destination history |
   | `destination_history_complete`, `in_flight_requests` | `true` and `0` |
   | `old_effect`, `new_effect` | `"absent"` for both exact provider operations |
   | `verified_sibling_ref`, `retained_fee_ref` | Exact independently verified references for 542 outputs 0 and 2 |
   | `evidence_sha256` | Lowercase SHA-256 of the retained evidence file |

   Supply the exact evidence file as the last argument to `prepare`. The signed
   instruction binds its digest, the confirmed state and a separately signed
   replacement preparation. The ledger transfers the revision/start lock and
   provider tail directly to the frozen 542 output in one transition. Both
   paid watermarks stay at their original values. The ordinary 542 settlement
   still requires a succeeded attempt with unique matching transfer evidence.

4. If the old transfer is verified, use `reconcile_verified`, set
   `attestation.old_effect` to its transfer reference, and include its exact
   canonical schema-2 evidence in the review's top-level `stripe_transfer`
   field. This finalizes the existing 508 attempt using its original terms.
   To prepare the ordinary accounting operation separately, supply a new
   confirmed snapshot and the exact signed resolution file.

   ```sh
   node intercom/scripts/fiat-preparation-recovery.mjs verified-settlement SNAPSHOT SIGNED_RESOLUTION NEW_FEATURE_FILE KEYPAIR
   ```

   Only this ordinary output settlement advances the paid watermark. Its
   advancement makes the old 542 start stale. A verified 542 effect without its
   canonical preparation, an ambiguous transfer, or a missing or renewed 508
   attempt leaves the case pending for independent review.

For a restart, retain and replay the same signed file. An exact duplicate
returns the existing result. A conflicting instruction or a changed observation
fails. Keep the local 508 batch waiver in place. Its fee has no proven starting
watermark for a recovery sweep. The signed fence also holds any outstanding 508
fee preparation, retention attempt and accounting operation, including a fee
attempt finalized before fencing. The repair leaves 508 open, and the CLI
holds the overlapping batch before entering the Stripe sender.
