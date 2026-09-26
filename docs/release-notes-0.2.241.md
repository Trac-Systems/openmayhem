# OpenMayhem Core 0.2.241

Valid terminal inference results remain deliverable while a transient receipt
settlement handoff is recovered from the durable job record. The gateway waits
for the exact job's existing reconciliation instead of reporting a provider
failure after output and a signed receipt have already been accepted. It does
not rerun inference or duplicate settlement.

The behavior applies to streaming and non-streaming requests across the shared
gateway execution path. Genuine model-output, request-contract, and permanent
settlement failures remain errors.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged; no recalibration is required.
