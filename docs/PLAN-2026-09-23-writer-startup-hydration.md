# Bound canonical writer startup hydration

Follow-up for the next Core update. Do not bundle this into the current v0.2.262 limited cutover.

## Observed defect

`intercom/src/admin-view-hydration.js` calls `view.createReadStream()` without a key range or entry limit every time the canonical admin writer starts. On 2026-09-22 the view reported `contiguousLength == length == 9,427,862`, yet startup still traversed the whole current view before the peer RPC became ready. The restart spent minutes in this phase. This is unbounded with ledger growth. The log describes the view's block length; it is not evidence that all 9.4 million records were loaded into RAM. Measure actual entry count, bytes read, peak RSS, and duration in the isolated proof.

## Proposed design

1. Determine what safety property the traversal supplies. Check the persistent view's signed length, contiguous length, and applied-view prefix proof against the canonical indexer after restart. Confirm whether a full read stream is required for contract execution or only warms locally available blocks.
2. If the persisted view is already contiguous and its canonical prefix proof matches, open the writer without traversing all keys. Read only the exact critical keys needed for admission and settlement; leave ordinary reads on demand. If this cannot be proven, do not declare the writer ready.
3. If a verified checkpoint is required, make it a derived, versioned cache bound to canonical length and Merkle hash. Resume from that boundary and replay only the tail. Never replace, truncate, or reinterpret the canonical indexer store. Reject a mismatched checkpoint and provide an explicit recovery path; do not silently accept a different fork.
4. Bound any remaining warmup by entries, bytes, time, and memory. Put optional warming behind readiness, with visible progress. Do not perform an unbounded full-view stream on the critical startup path.

## Acceptance

- Restart with a large existing view and prove identical canonical prefix hash before and after, including a nonzero local fork counter on a sparse peer.
- Demonstrate startup time and peak memory remain bounded as ledger length grows; compare at two substantially different view lengths.
- Exercise exact-key reads, a new ledger append, one settlement receipt, and an epoch after restart. Confirm no duplicated execution or changed contract digest.
- If the proof fails, admissions stay closed and the reason is explicit. Never repair by wiping or copying the indexer store.

This is a Core/Intercom startup-path change and must follow the established limited matched rollout before any wider deployment.
