# Canonical contract history replay

Admission uses the current contract version. Replaying accepted history is a separate
operation: a newer contract must reproduce the older transition, including its
pricing and receipt rules, instead of running that input through newer business logic.

The Feature and Tx consensus handlers first perform their existing signature, MSB
payment/content, identity and duplicate checks. They can then create a one-use,
process-local replay capability. It is bound to the exact operation object, serialized
full envelope and executing storage batch. The original authenticated writer block
must contain the same envelope. The signed canonical read view must contain either:

- Feature: `sh/<signature>` and a matching `fr/<signature>` result with the same
  signature, feature key and address.
- Tx: `tx/<transaction>` and its `txi/<index>` record with the same transaction,
  invoker, validator and full dispatch.

A dispatch field or RPC caller cannot provide this capability. Unaccepted, altered,
optimistic, forged, reused and cross-batch evidence cannot enable compatibility.
Only v23, v24 and v25 are eligible. Fresh operations continue through current admission;
the existing evidence-bound prepared checkpoint path remains separate.

Accepted historical calls execute serially with retained source implementations:

| Contract | Source release | SHA256 of retained source |
|---|---|---|
| 23 | v0.2.191 (identical in v0.2.183–v0.2.191) | `365330d5a94be2c8a3afdbeb2766e052c1ab3324f39880d7940d0a9a91b9a0bc` |
| 24 | v0.2.193 (identical in v0.2.192–v0.2.193) | `695c8010aa8f61fcedeb277f4251f0c6bd670bdb1c2a133b2da175900f45dd08` |
| 25 | v0.2.242 | `31d570bd8ab6e89b469e67f1d035f1cb943ff99ded8ed7824b490841ed733a4a` |

These sources use no mutable protocol business helpers. Their only protocol access
is wallet signature verification and immutable subnet/MSB network identity for
admin signing contexts. The wallet implementation is unchanged across these source
releases. Each retained contract supplies its own version, signing domains, schemas
and economic methods. The current release identity includes all retained files and
the replay admission/consensus transport sources.

Regression fixtures use real Autobase/Corestore history, close and reopen persisted
stores, and replay signed Feature and MSB-verified Tx inputs into a fresh Hyperbee.
They compare the complete state and Hypercore tree hash with the original-version
writer. The Tx fixture preserves a historical pricing bound which current admission
would reject; the Feature fixture writes an economic rate. Duplicate replay changes
neither state nor tree length. Negative cases cover missing and altered acceptance
records, changed writer bytes/version, fresh old dispatches and forged capabilities.

## Applied-state verification

`GET /v1/status?applied_view_length=<canonical-boundary>` returns
`consensus.applied_view_proof` with the requested length, materialized apply length,
fork and prefix tree hash, or null if that prefix is not applied. It reads the atomic
apply view rather than the sparse signed read session. Compare this hash with the
canonical writer's proof at the same boundary. A large `subnetSignedLength` alone
does not establish replay completion.

Also check `consensus.base.caught_up`, idle advancing/draining/applying state,
`consensus.contract.execution_queued`, and `consensus.contract.replay.active`.
Observe repeated idle snapshots with the same process identity, no restart increase
and no fatal replay error for at least 60 seconds before admission. The materialized
view length is `consensus.apply_state.view.length` (also the named view's
`core_length`); the sibling `views[].length` is a cached system entry and may lag.
