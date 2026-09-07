# Writer settlement and paid checkpoints

Core v0.2.174 uses contract revision 22. Upgrade all participating peers before
enabling the new writer operations. Keep the existing wallet, stores, model
configuration, catalog and economic parameters.

## Two independent clocks

The economic epoch remains governed by `epoch_seconds` (currently 3,600 seconds).
The settlement timer polls every five seconds. At the boundary, `admin epoch-freeze`
atomically records `epoch/freeze/<epoch>` and advances `receipt/ingress`. Subsequent
final receipts and cancellation closures enter the next batch. Every apply page
uses the closed index and its original timestamp. Payout processing remains separate.

The paid checkpoint worker runs inside the admin Intercom peer. It records one
UTC thirty-second slot even when there are no requests. It neither creates usage
nor advances payout maturity. The worker can keep checkpointing while settlement
is processing a large batch.

## Activation

On the writer only, supply these environment variables to its Core supervisor:

```text
MAYHEM_WRITER_CHECKPOINTS=1
MAYHEM_WRITER_CHECKPOINT_DIR=/opt/mayhem/.mayhem-local/writer-checkpoints
MAYHEM_WRITER_CHECKPOINT_RESERVE_AU=5000000000000000000
```

The directory must persist across release changes. The default reserve is 5 TNK.
The worker holds an operating-system file lock and refuses a second spender.
Provider, buyer and helper peers leave this setting disabled.

Install the updated `mayhem-epoch-cadence.timer` and ensure the cadence/finalizer
use the matching release's `scripts/ops-freeze-epoch.py` and `mayhem` CLI. Resume
existing partial pages before closing a new batch. Retain all snapshots, holds,
receipt identities and payout records during the rollout.

## Payment and recovery evidence

Each slot first prepares an immutable canonical snapshot at
`checkpoint/prepared/<slot>`. Its paid `state_checkpoint` command references the
snapshot digest. The contract rejects this command through free Features,
including signed admin Feature envelopes.

The journal persists the exact signed MSB payload before broadcast. An unknown
response triggers reconciliation of the same transaction hash. After MSB payment,
missing subnet application is recovered with the original proof, without another
payment. A validator context change permits replacement only after signed MSB
system/view evidence establishes non-execution; the retired attempt and that
evidence remain in the journal. An unsigned context change leaves the attempt queued.

The canonical success record is `checkpoint/slot/<slot>` and the latest pointer
is `checkpoint/current`. A completed local artifact contains the paid operation,
signed MSB view length, encoded consensus evidence and matching subnet record.
An RPC acceptance, a free Feature result or a hexadecimal identifier alone does
not prove payment.

Do not delete `state.json` or the checkpoint directory to clear an error. Missing
or damaged payment history requires restoring the retained journal. On orderly
shutdown, the worker stops before the peer closes. Before a later contract
revision, disable new checkpoint scheduling and reconcile its pending paid attempt
under the current revision before replacing the peer.

## Cadence, funding and monitoring

`GET /v1/status` includes `writerCheckpoint`: status, queue size, active transaction,
last completed slot, funding and the latest error. Durable details are in
`state.json` and `completed/<slot>.json` in the configured directory.

The current fee is 0.03 TNK per paid transaction: 2,880 scheduled slots per day
cost 86.40 TNK. Other operations and catch-up transactions use additional runway.
The worker reads the existing peer's fee address and its signed MSB balance.
Funding exhaustion pauses paid broadcasts and retains due slots.

Slots remain independent of traffic. Confirmation can be delayed by outages,
insufficient funds or network finality. Catch-up is bounded, and delayed slots
record their actual observation time. Report missed and late confirmations
separately from the target of two paid transactions per minute.

## Release acceptance

`node intercom/scripts/writer-checkpoint-msb-smoke.mjs` creates a disposable
development MSB ledger and verifies actual fee deduction, signed payment proofs,
lost-acknowledgement restart, missing subnet application recovery and replay
protection for three checkpoints. Its subnet storage is an in-memory fixture
using the real contract and transaction verifier. It does not establish public
network cadence. `writer-checkpoint-runtime-smoke.js` separately checks journal
durability and the spending lock under Pear/Bare.

Verify continuous receipt ingress across cutoffs, final/cancellation boundaries,
partial-page restart, immutable billing terms and exactly-once accounting. Verify
zero-traffic paid slots, restart before/after broadcast, lost acknowledgement,
MSB payment without subnet application, validator context changes, clock rollback,
funding exhaustion and the spending lock.

Run thirty minutes of live idle/active observation, verify every paid MSB hash and
canonical checkpoint, and continue the twenty-four-hour cadence observation.
Track settlement advancement independently from paid checkpoint confirmations.
