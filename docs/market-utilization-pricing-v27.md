# Contract v27 utilization pricing rollout

This change requires a coordinated Core contract upgrade. Contract v27 adds signed `compute_ms` and `capacity_slots` to schema-12 receipts and replaces previous-epoch activity momentum with absolute slot utilization. Prices move +10% at or above 80%, -10% at or below 20%, and hold between those thresholds. Existing 25%-400% seed bounds remain.

## Prepare offline

Export a complete canonical snapshot with `at`, `epoch_apply_state`, `pending_price_commits`, `modelrefs`, `enclaves`, and the full `prices` array. Finish any pending paged epoch and resolve prior-version nonempty price commitments first.

```sh
node intercom/scripts/prepare-market-activity-upgrade.mjs canonical-snapshot.json unsigned-plan.json
```

The generator writes unsigned `migrate_market_pricing` batches of at most 128 markets. It does not sign or submit anything. Verify `required_final_index` against the canonical store before resuming settlement.

## Coordinated cutover

1. Build and authenticate one release containing contract v27, receipt schema 12, the writer recompute changes, and all gateway/provider receipt changes.
2. Drain a canary gateway and provider without discarding pending receipts or outboxes. Upgrade the canonical writer and canary components under the established contract-transition procedure.
3. At a completed epoch boundary, submit every reviewed `migrate_market_pricing` batch. Confirm the activity index, migration-v3 record, contract digest, and canonical fork.
4. Prove streaming and non-streaming receipts, checkpoint monotonicity, multi-slot capacity, final epoch recompute, the 20%/80% boundaries, and retained v26 receipt recovery on the canary.
5. Only after that proof, drain and upgrade every remaining gateway, provider, helper, payment watcher, and settlement worker to the identical release. Preserve all stores and outboxes.
6. Verify every process reports the same release/hash and every follower store is on the writer/indexer canonical fork. The writer/indexer store is the source of truth and must never be wiped.
7. Resume normal admissions and verify the first completed production epoch publishes schema-3 utilization derivations.

If canary proof fails, keep the rest of the fleet on the prior release and correct the candidate. Once v27 operations or schema-12 receipts are accepted, use a state-aware forward correction rather than blindly rolling the contract back.

Public release notes must describe behavior without naming hosts, operating systems, credentials, or internal fleet topology.

Retained schema-10/11 receipts remain payable after cutover. Because they predate signed `compute_ms` and `capacity_slots`, any market containing one records a legacy-evidence hold and leaves price unchanged for that epoch. Never synthesize utilization for them. Normal utilization pricing resumes automatically once an epoch contains only schema-12 receipts.
