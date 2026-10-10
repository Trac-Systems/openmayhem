# Retained provider data-handling declarations

The shared setup store can prepare, explicitly sign and inspect a provider's
data-handling promises. CLI and the protected local dashboard share these
actions. A first reviewed managed Run includes the exact signed declarations
in its immutable configuration. Updating an already installed controller's
declarations remains a separate lifecycle operation, not an automatic restart.

Configure the registry trust anchor in the protected wizard configuration as
`declaration_registry: {"origin":"https://api.openmayhem.ai"}`. Do this only
after the intended registry is published at that origin. First setup accepts
`--declaration-registry-file`; dashboard bootstrap accepts
`--proxy-setup-declaration-registry-file`. These read an owner-only JSON file
containing that object. Literal loopback HTTP is available only with explicit
`local_loopback_http: true` for local acceptance. Browser actions cannot select
an origin, file, wallet, or arbitrary signing body.

Use **Choose published fields** in the dashboard or **j** in the CLI wizard.
Fields are paged from one pinned release, 32 per page, and only compatible
filter declarations are selectable. Pages replace each other; selected claims
are retained up to the declaration's 32-field bound. No full registry/history
walk or background polling is introduced. Boolean, enum, set, integer, exact
decimal and text values follow the published schema. Missing fields stay
unknown; choosing a field does not certify remote compliance.

Set an explicit expiry and review all chosen values. **Sign reviewed
declarations** (CLI **y**) signs that retained plan. Refreshing/reopening setup
does not sign, pay, publish or run anything. Unattended clients can use the same
typed `declaration_fields`, `declaration_plan` and `confirm_declaration` actions
through the existing protected action-file interface. Exact definitions are
resolved again from the pinned release when the plan is prepared.

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
`declared_not_verified` assurance. After an exact Run configuration is installed,
the flow reports `configured_in_controller`; this does not attest process health
or current routability. Otherwise `installed_in_runtime` remains false. A provider
signature proves who made a promise; it does not prove remote compliance, model
identity, operator review or stronger assurance. Routing continues to enforce
its existing exact-subject, expiry, freshness and conditional-rule checks.

Focused acceptance uses the real registry reader against a loopback publication
fixture, protected setup files and wallet signatures. It covers all four proxy
endpoints, exact replay after expiry, explicit renewal, invalid fields/types,
wrong wallets and changed routes, while asserting that the original draft and
its financial terms are unchanged. It is not production payment or deployment
evidence. Additional checks exercise the shared Flow's pinned discovery, typed
review, existing wallet, recovery during registry outage, and rejection of
injected destinations/signing bodies. The managed loader proof checks that
adding a declaration invalidates the old Run review without changing the
financial draft and preserves the exact signature through retained Run recovery.

Current lifecycle limitation: signing a renewal does not replace an installed
immutable Run. Setup reports its pending activation and rejects a conflicting
replacement; operators must not delete the original run record to bypass that
guard. Controlled declaration activation/withdrawal for an existing controller
is still a release requirement. A signed promise's expiry is never extended by
inspection or a restart.
