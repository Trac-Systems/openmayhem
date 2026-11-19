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

### Submitting a prepared fence or resolution

On the writable admin host, use its verified loopback RPC base ending in
`/v1`. Work in the private evidence directory. Keep the fence and resolution
in separate files. Submit one stage at a time through the ordinary feature
endpoint. Once the fence is confirmed, follow the drain and processor evidence
steps above before preparing the resolution.

```sh
umask 077
curl --noproxy '*' --fail-with-body --silent --show-error --max-time 30 \
  --header 'Content-Type: application/json' --data-binary "@$signed_file" \
  --output "$reply_file" "$writer_rpc/contract/feature"
```

An HTTP 200 can carry a rejected or pending append. Confirm the economic
effect in the signed state before continuing, including after a lost response
or restart. For a fence, use `record_kind=fence`. For either resolution use
`record_kind=resolution`. This read checks the complete signed instruction.

```sh
old_operation=$(jq -er '.value.old_operation' "$signed_file")
curl --noproxy '*' --fail --silent --show-error --max-time 10 --get \
  --data-urlencode 'confirmed=true' \
  --data-urlencode "key=payout/recovery/fiat/$old_operation/$record_kind" \
  --output "$readback_file" "$writer_rpc/state" &&
  jq -e --slurpfile signed "$signed_file" \
    '.confirmed == true and .signed_length > $signed[0].value.signed_length
     and .value.feature_key == $signed[0].key and .value.value == $signed[0].value' \
    "$readback_file" >/dev/null
```

A confirmed null means the effect is still absent at that read position.
Check the recorded append result and the writer's progress. If resubmission is
needed, submit the same saved bytes. After a confirmed `supersede_absent`,
the existing worker timer resumes 542 and reconciles its transfer.

## Contract upgrade prerequisites for existing proxy deployments

1. Drain new publications on the current writer and use its normal recovery to
   resolve queued operations. After stopping its owner, preserve the journal
   with the rest of the store. Contract 31 can reopen an empty v30 journal for
   the same writer key, fork, administrator, network and bootstrap keys. A
   journal with pending entries must finish recovery with its original owner.
2. At the cutover, read the confirmed registry configuration and policy head at
   one signed length. For an existing v30 registry, submit one ordinary
   admin `proxy_policy` action of kind `configure` in the v31 context, with the
   current head revision plus one. Copy the entire saved configuration and
   change only `contract_version` to 31. Verify the updated head, registry
   configuration and catalog network projection from confirmed state. On a
   lost acknowledgement, retry the identical operation after reading the head.
3. Align the gateway, provider control, buyer and admission worker network
   configurations. For the website, align `PROXY_ADMISSION_AUTH_CONTRACT_VERSION`,
   the network in `PROXY_ADMISSION_ENROLLMENT_CONFIG` and
   `PROXY_FAMILY_ADMIN_CONTRACT_VERSION`. Verify the versioned store upgrade,
   replay protection and fresh route evidence before reopening admission.
   Recheck pending invoices, refunds and permits against their original payment
   evidence. Existing providers keep their entitlement, market ID and price.
4. For systems that previously received a targeted payout-worker update, check
   the effective service command, source directory, executable and imported
   worker files against the staged release. A lower-priority override may still
   select an older installation when the temporary override is removed.
   Preserve settlement cursors, timer scheduling and deposit watcher enablement.
   Verify the files used by the restarted processes and the next scheduled
   payout invocation.
5. Verify a fresh quote and reservation with v31 terms and reconciliation of a
   previously accepted v30 request with its original signed terms. Preserve
   all three payment rails, prior receipt commitments and account watermarks.

The gateway opens each projection database under its existing lock. For the
same network and bootstrap keys, it archives the v30 catalog tables in place
and builds the v31 catalog from fresh signed pages. Allow disk space for both
snapshots. Presence keeps its replay watermarks and needs a new signed
heartbeat. Conformance keeps its original signed rows and requires current
evidence for routing. If startup stops between database files, restart with
v31. Restoring an old file after admitting traffic could rewind replay
protection. Verify the new heartbeat, conformance (where configured) and an
actual routed request before announcing availability.
