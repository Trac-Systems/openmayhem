# Retained provider data-handling declarations

The shared setup store can prepare, explicitly sign and inspect a provider's
data-handling promises. This is the authoring/storage foundation. CLI and
dashboard controls and activation in an existing managed runtime are not yet
wired; this document does not claim those surfaces are complete.

`Store::plan_data_handling` takes the current draft revision, the last signed
declaration revision, an explicit expiry, and typed operator choices. Definitions
must come from the configured trusted registry reader's pinned `Definitions`
snapshot. The caller cannot substitute browser JSON for that snapshot. Only
published filter fields applicable to the selected endpoint are accepted;
values must match their exact schema. No default privacy claim is inferred.

The store derives the subject from the saved provider/network, market,
membership digest, endpoint contract, recipe and connection revision. The
operator cannot supply another subject for signing. The reviewed plan retains
the registry release, definition digests and the exact issue/expiry times.

`confirm_data_handling` accepts the retained plan digest and the existing
unlocked wallet authority. It rejects a changed draft, changed connection,
wrong wallet, expired plan or superseded revision. A repeated confirmation of
the last signed plan returns its exact original signature, including after
expiry. It never silently extends the expiry or signs another revision.

Renewal and edits require a new explicit plan and confirmation. Preparing one
does not overwrite the last signed record. Storage uses the existing protected
directory lock and atomic fsynced writes, keeping bounded pending and signed
files. The original market, rates, admission fee, draft and financial state are
unchanged. No probe, model generation, ledger operation or runtime restart is
performed by these methods.

Inspection reports `needs_confirmation`, `signed_not_installed`, `expired`,
`not_yet_valid` or `configuration_changed` as applicable. It explicitly reports
`declared_not_verified` assurance and `installed_in_runtime: false`. A provider
signature proves who made a promise; it does not prove remote compliance, model
identity, operator review or stronger assurance. Routing continues to enforce
its existing exact-subject, expiry, freshness and conditional-rule checks.

Focused acceptance uses the real registry reader against a loopback publication
fixture, protected setup files and wallet signatures. It covers all four proxy
endpoints, exact replay after expiry, explicit renewal, invalid fields/types,
wrong wallets and changed routes, while asserting that the original draft and
its financial terms are unchanged. It is not production payment or deployment
evidence.
