# Retained provider data-handling declarations

The shared setup store can prepare, explicitly sign and inspect a provider's
data-handling promises. CLI and the protected local dashboard share these
actions. A first reviewed managed Run includes the exact signed declarations
in its immutable configuration and pins a protected declaration source. Later
explicitly signed renewals/withdrawals update that source without restarting
the controller, model, or changing any accepted job's terms.

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
are retained up to the declaration's 32-field bound. Browsing does not walk or
poll the registry history. Boolean, enum, set, integer, exact
decimal and text values follow the published schema. Missing fields stay
unknown; choosing a field does not certify remote compliance.

Set an explicit expiry and review all chosen values. **Sign reviewed
declarations** (CLI **y**) signs that retained plan. Refreshing/reopening setup
does not sign, pay, publish or run anything. A configured running controller
observes explicitly signed changes on its local metadata polling cadence.
Unattended clients can use the same
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
`not_yet_valid`, `configuration_changed` or
`superseded_requires_explicit_renewal` as applicable. It explicitly reports
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

Managed activation uses one bounded protected signed file and one durable
last-observed checkpoint per setup draft. A single background task checks the
configured sources every five seconds, with no overlapping reads and a
nonblocking setup lock. It does not walk receipts, prices, ledger history or
registry pages. Descriptor reads use only bounded memory and enforce both signed
expiry and a fifteen-second monotonic read lease. A stopped/stalled poller cannot
leave declarations eligible indefinitely; consumer freshness checks still apply.
Updates are not instantaneous across cached consumers.

The checkpoint is fsynced before exposing a higher revision. Regression and
same-revision equivocation are rejected, including after restart. An observed
expiry is latched; rereading a signature cannot extend its lifetime. Authoring
advances the durable high-water too. Missing, corrupt or inaccessible source
files remove declaration eligibility without falling back to static claims or
the checkpoint, interrupting accepted inference, or changing prices/payments.
Normal reads cause no disk writes; new revisions and first observed expiry do.
Managed health shows bounded source-error categories and current declaration
identity. Setup's configured/observed indication is not a live health attestation.

Use **Review withdrawal of all claims** (CLI **w**) and then the separate signing
confirmation to replace the last claims with explicit Unknown values. This works
during registry outages, preserves field meanings and needs no protocol change.
A restored source older than the observed checkpoint requires explicit renewal
from the current registry before withdrawal; it cannot silently reuse an old
claim set. Subsequent Run recovery retains its original configuration, process
identity, capacities, consumed probe budget and financial terms. It does not
rebuild the Run digest from renewed declarations.

Manually configured controllers without a declaration source retain startup-only
declarations; setup does not mutate such an existing configuration silently.
The source and checkpoint remain owner-protected local state. Their deliberate
deletion by the operating-system owner is outside the anti-rollback guarantee.
