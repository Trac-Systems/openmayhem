# Platform proxy commercial policy

`platform-commercial-policy-v1.json` records the platform owner's approved
starting rules. It is a source reference, not a runtime configuration file or an
activation switch. Installing or upgrading Core does not load it automatically.
No native prices, reservations, payout rules or health defaults change with it.

For an authorized deployment, use the existing explicit configuration paths:

1. Register `settlement_policy` with its exact `settlement_policy_hash` using the
   existing authenticated `set_settlement` operation. Scope it to the intended
   proxy network. Both buyer and provider must accept the same policy; do not
   replace another operator's selected policy silently.
2. Copy `settlement_policy` and the three `buyer_lifetimes` fields into the
   platform proxy buyer configuration. The lifetime fields are top-level fields
   in that configuration, not a nested `buyer_lifetimes` object. Preserve its
   network, wallet, resource limits and other explicit settings. Record the
   matching policy revision. Do not edit already accepted terms or receipts.
3. Copy the two `admission_quote` fields into the API enrollment configuration's
   `policy` object. Bind the independently reviewed network, issuer, receivers,
   payment identities and retained discovery cursors there. This reference does
   not supply those operational values or enable collection.

The settlement hash binds only `settlement_policy`. Purchase lifetimes become
part of each signed acceptance; quote timing and amounts become part of each
immutable admission invoice. Do not use that one hash as a signature of this
entire reference file or as a network identity.

Verified completed, partial, cancelled and valid model-refusal outcomes may
settle measured usage under the original rates. HTTP/authentication/transport
errors are not completed model refusals. Running financial checkpoints are
disabled. An expired uncertain attempt releases only its unfinalized exposure;
it cannot be blindly dispatched again. Definitive nonexecution evidence uses
the existing reconciliation path. Finalized charges remain payable.

Reservation coverage is 24 canonical billing epochs plus 6 receipt-grace epochs.
Read the network's actual epoch duration before activation. This is approximately
30 hours with hourly epochs, not a wall-clock guarantee or a reference to the
network transaction cadence. A stalled canonical epoch does not authorize local
expiry. Acceptance is limited to the original billing epoch. These limits do not
cap the lifetime of a project containing many separately authorized requests.

Admission quotes last 60 minutes and use rates no older than 5 minutes. Top-ups
use the same amount and original deadline. Delayed confirmation never reprices
a matched transfer. Truly late funds or unprovable transfer timing require
reconciliation/return handling. Excess returns use the original rail, verified
recipient authority and platform-paid fees; they do not create retail credit.

The website source retains an identical reference at
`docs/data/proxy-platform-commercial-policy-v1.json`. This policy still requires
the separately authorized release, disabled configuration review and actual-rail
acceptance before public activation. Local fixtures are not live payment proof.
