# Contract v25 activity pricing rollout

This source change requires a coordinated contract/Core upgrade. It has not been deployed. Fresh operations use contract version 25; recovery preserves original signed v23 and v24 receipt envelopes and feature keys. Receipt schema 11 is unchanged.

Price direction compares immediately consecutive epochs; EMA remains telemetry only. Less work falls, more work rises, and equal nonzero work holds, subject to hard bounds and integer price granularity. Every initialized empty epoch continues stepping down toward the hard floor, including zero-to-zero epochs.

The controller and calibration schema are specified in [the pricing reference](knowledge/market/pricing-controller.md). Signed catalog inventory currently contains 22 model rows: 11 workflow, six text-generation and one each image-generation, video-generation, TTS, STT and music-generation. The inventory helper derives each row's billable units, including legacy text rate fields. Existing machine-readable catalog fields do not provide a complete authoritative reference-work calibration for every axis. Prose measurements depend on their execution conditions and must not become guessed universal capacities. All existing references therefore have an immediate dimension-relative fallback; admin-signed measured weights can be introduced later.

## Offline preparation

Export a complete canonical snapshot containing `at`, `epoch_apply_state`, `pending_price_commits`, maps `modelrefs` and `enclaves`, and the full `prices` schedule array. Include all active base and context schedules, including dormant markets and due pending schedules. Export completeness must be checked against canonical state; a local JSON file cannot prove its own completeness.

Finish any partially applied epoch under the old contract. Resolve all prior-version nonempty-price-root commitments before upgrade: v25 does not reinterpret old dollar-based fraud proofs. The generator rejects a nonempty `pending_price_commits` list or pending paged apply.

Run locally:

```sh
node intercom/scripts/prepare-market-activity-upgrade.mjs canonical-snapshot.json unsigned-plan.json [calibrations.json]
```

The optional final argument is a map of model IDs to measured calibration objects. The tool validates complete positive unit coverage, preserves reference prices, emits an inventory and unsigned admin operations, and writes a fresh private output file. It performs no signing or network mutation.

## Coordinated migration

1. Review the inventory, complete price coverage, source hashes and unsigned payload. Preserve the prior state snapshot and binaries.
2. Upgrade the consensus contract and compatible Core clients to v25 at a completed epoch boundary, before applying the next epoch. Receipt ingress recovery supports both 23 and 24; do not rewrite signatures.
3. Submit every admin `migrate_market_pricing` batch (at most 128 markets per batch, 5000 indexed markets total). The contract validates each row against active enclave, price seed, model reference and context state, rejects partial epoch migration, and records repairs. Repeating batches is safe.
4. Verify canonical `market/activity/index` covers `required_final_index` exactly for the active exported schedules. Raw unsafe active/pending bounds are repaired to the safe defaults; original records are retained in the migration audit, and immutable parameter-update history is untouched. Unsafe pending bounds are already suppressed on v25 reads, before migration writes.
5. Apply optional signed model-reference calibrations. Omission preserves calibration and explicit null clears it. Adding, changing or clearing weights holds one epoch while establishing the new baseline. Keep signed calibration evidence available to auditors.
6. Resume settlement. Confirm baseline holds once, subsequent single-provider rise/fall, dormant zero-demand updates, hard bounds and original session locks. Active step/EMA parameters remain delayed admin changes; six monetary-utilization knobs remain readable only as deprecated compatibility fields.

Do not roll back the contract blindly after v25 state or operations have been accepted. A state-aware forward correction is required. A bad calibration itself is reversible through an admin null update without replacing history.

## Evidence and limits

Canonical signed usage roots already commit workload dimensions. The recompute script exposes `market_activity` for audits; submitted `market_usage` cannot inject workload counts. Paged settlement accumulates canonical work once, validates every page, and commits consensus-derived price evidence at completion. Empty seals commit their zero-activity price evidence too.

Explicit nonempty price commitments support one market and at most 128 canonical final receipt heads. The contract verifies the complete frozen usage root and pins the expected work-based derivation and baseline at commitment. An honest root cannot be challenged; a contradictory root can be challenged even after model-reference changes. Larger settlements use bounded receipt pages with consensus-derived price updates, rather than externally supplied price roots. Historical AU-only price commitments must be resolved before upgrade. Existing over-credit proof scope is unchanged.

The activity index is bounded at 5000 markets and must be provisioned before traffic resumes. Retired market history remains immutable; future topology/context changes require active schedule/index review. Reaching the bound rejects new indexed markets before price or settlement writes. There is no automatic proof that all possible future market schedules were included in an old snapshot.

Reference work is not hardware occupancy. Dimension fallback equal-weights relative changes and may respond differently when the workload mix changes; measured admin weights improve comparability. Completed activity cannot reveal saturated unmet demand. Self-funded wash work remains possible and is limited by settlement costs, step bounds and hard price bands, not an unverifiable claim about intent.
