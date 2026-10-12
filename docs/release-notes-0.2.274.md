# OpenMayhem 0.2.274

## Buyer gateway lifetime reservation cap

Core 0.2.272 and 0.2.273 gateways using the durable API-key budget store count
every retained reservation toward `max_budget_reservations` (serialized as
`max_reservations` in the activation file). Completed and released reservations
remain stored to prevent duplicate accounting, but incorrectly occupy admission
capacity forever. With the usual 10,000 setting, a gateway can therefore stop
admitting requests after 10,000 lifetime reservations, even with sufficient funds,
almost no pending work and healthy available providers.

The failure is local to the buyer gateway and occurs before provider dispatch.
The characteristic HTTP 503 message is:

> gateway key budget unavailable; retained exposure requires recovery

This message alone is not a diagnosis: an unavailable accounting database can
produce it too. Do not delete or reset the budget database to recover capacity;
it contains spend totals, unresolved exposure and replay protection. The defect
can affect FIAT, TNK and TAP independently, native and proxied requests, and every
model modality that passes through this accounting authority. Streaming and
non-streaming requests share it. Provider availability does not detect this local
buyer admission failure.

The fix counts only the existing pending-reservation index when admitting work.
Settlement and release free a slot; an unresolved reservation keeps its slot and
financial exposure. Verified proxy non-admission fences remain writable even at
capacity. Completed/released records still prevent duplicate admission/accounting,
and released native sessions can still receive a late verified receipt.

Authenticated gateway token status now includes `budget_capacity` with
`pending_reservations`, `max_pending_reservations` and `retained_reservations`.
Reservation-capacity errors, account-capacity errors and storage failures have
distinct messages. Customer monetary budgets remain unchanged.

The check uses table metadata inside the existing atomic admission transaction;
it does not scan requests or receipts. The database cache remains 4 MiB. Durable
replay records still grow on disk; this release does not prune financial history
or claim bounded total disk usage.

## Operator update

Update and restart each affected buyer gateway process, including separately
configured payment rails and embedded gateways. Keep its existing configuration,
key-budget database, wallet, peer and ledger stores. No database migration, ledger
contract change, model recalibration or provider-fleet rollout is required.
The ledger contract remains version 30 with the same code digest.

The existing limit now means simultaneous pending reservations. An emergency
increase made to work around the old lifetime cap may be returned to its intended
pending-work limit after installing this fix. Where both a proxy buyer config and
an activation file specify the limit, keep them matching. Ensure supervisor and
service startup paths use the updated binary so a later restart cannot restore
the defective version.

Regression coverage includes repeated native/proxy settlement and release far
beyond capacity, a retained unresolved request during turnover, restart with more
history than capacity, late signed native charges, replay rejection, and atomic
admission when native and proxy requests contend for the last slot.
