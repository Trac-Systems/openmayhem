# Recovering an interrupted proxy monitoring probe

A monitoring timeout or controller restart does not prove that the upstream
stopped working. The controller retains the probe's capacity and its spent
monitoring allowance instead of silently sending it again. `retained_work` in
the bounded recovery diagnostics identifies this case.

Stop the affected proxy controller before these commands. The existing capacity
store's exclusive lock rejects inspection or resolution while it is open. Keep
the original configuration, state directory and provider wallet; do not delete,
copy over, reset or recreate stores. These commands do not touch ledger state.

```sh
mayhem provider proxy recovery-status --config /private/provider.json \
  --home /private/provider-home
```

This inspects only configured connection groups, prints their probe identifiers,
upstream fingerprints and cumulative monitoring budgets, and applies the normal
restart fence. It does not start models, publish availability, reset allowance,
or resolve customer purchases.

Independently verify that the **original upstream request** completed, was
cancelled, or never executed. A local timeout, an unrelated successful request,
or a locally empty queue is insufficient. Use upstream execution records or
authoritative backend state. Retain that evidence privately, then create an
owner-only confirmation file (mode `0600` inside a private directory):

```json
{
  "schema_version": 1,
  "probe_id": "<exact probe ID from recovery-status>",
  "connection_digest": "<exact upstream fingerprint from recovery-status>",
  "evidence_digest": "<64-character SHA-256 of retained upstream evidence>",
  "upstream_stopped": true
}
```

```sh
mayhem provider proxy resolve-recovery-probe --config /private/provider.json \
  --home /private/provider-home --confirmation /private/probe-confirmation.json \
  --confirm-upstream-stopped
```

Use the normal wallet-selection arguments if the provider uses an explicit
keypair. Never put passwords or upstream API keys in logs or confirmation files.
The command checks the owning wallet, configured upstream, exact retained probe,
route membership and uncertain phase. A different retained probe is rejected;
repeating the completed resolution with no retained probe makes no new release.
The stored completion binds the operator confirmation to its evidence digest.

This releases **only the confirmed-stopped monitoring probe**, not paid requests.
Already allocated monitoring cost and attempt count remain consumed. No healthy
measurement is invented. Restart the controller with its original state and
require a fresh successful configured probe before advertising readiness. If
the cumulative monitoring allowance is exhausted, any increase must be explicit;
restarting does not renew it.
