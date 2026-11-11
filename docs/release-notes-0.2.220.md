# OpenMayhem Core 0.2.220

- Recover expired inference reservations from canonical ledger state even when a gateway's local job record is unavailable.
- Preserve any confirmed partial receipt when closing an expired reservation, so delivered work remains accounted for.

This release does not change the Intercom contract.
