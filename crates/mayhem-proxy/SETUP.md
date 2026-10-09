# Local provider setup drafts

The shared `mayhem_proxy::setup` library saves and reviews an explicit provider
declaration before any wallet, upstream connection, payment or publication is
opened. The CLI is a thin client of this same state implementation:

Private file verification and locking are currently implemented for Unix.
Other platforms fail closed; Windows permissions and setup remain unfinished.

```sh
mayhem provider proxy setup create --directory /absolute/private/setup --input /absolute/private/declaration.json
mayhem provider proxy setup inspect --directory /absolute/private/setup
mayhem provider proxy setup check --directory /absolute/private/setup --expected-revision 1
mayhem provider proxy setup update --directory /absolute/private/setup --expected-revision 2 --input /absolute/private/revised.json
```

The setup directory must already exist with owner-only permissions. Inputs and
connection files must be private regular files. Symlinks, hard links, unsafe
permissions, oversized documents and unknown fields are rejected. Relative
connection-file references resolve against the declaration file. Credentials
remain references in the private connection configuration; these commands never
read their values, unlock a wallet, contact a backend or start a process.

The version-1 `setup::Input` records the explicit network/provider identity,
connection file, private adapter snapshot, public market/membership, one to16
offers, create/join selection, declared operation sequence and public settlement
policy. One draft selects one endpoint adapter and one membership; its offers
can cover multiple explicitly declared submarkets. All four current endpoint
families and custom endpoint contracts use the same validators. Membership
contract/recipe, connection revision, metering, identity, rails and every offer
rate must agree. No category name, endpoint guess or upstream model label fills
missing fields or grants an assurance level.

Each directory owns one stable random draft ID and a monotonic revision.
Create refuses an existing draft. Update and check require the exact current
revision. An update always invalidates the earlier local check. Changing the
network/provider identity requires a separate setup instead of transferring an
existing draft ID. The store uses a nonblocking process lock, bounded files,
file sync and atomic replacement followed by directory sync. Interrupted
temporary writes never become authoritative on resume. Corrupt current files
are not silently replaced. An uncertain post-rename result requires inspecting
the original draft; it is not permission to create another invoice or identity.

`inspect` and every successful mutation return only the public `Review`.
It contains public market/membership/offer and endpoint-contract data, policy,
draft identity/revision and explicit state. Private connection paths and
fingerprints, upstream model mappings, local resource limits, URLs, credentials
and credential references are omitted. Recipe hashes and connection revisions
already required by the public membership remain present.

`unchecked`, `structurally_valid` and `recheck_required` describe local structure
only. Inspection rechecks the current connection fingerprint; file drift removes
the unsigned admission handoff until an explicit update and check. The review
always reports operator-declared claims, probes not run, admission not checked,
publication not submitted and serving not started. The optional handoff is the
exact unsigned initial `ProxyOperation` and its canonical digest, for the later
identity/invoice workflow to bind. It is not a permit, payment request, proof of
provider control, current sequence or evidence of unused admission entitlement.

The state library and unattended CLI are a setup foundation, not the complete
interactive/dashboard wizard. Next wiring must run explicitly approved bounded
probes through existing probe accounting, read canonical provider admission and
sequence before requesting an invoice, retain the original invoice/evidence,
verify a permit bound to the exact initial operation, and submit the typed
provider-signed operation through the existing publication journal/gate. A paid
or pending invoice cannot be replaced merely because a CLI request was lost.
Only confirmed canonical admission/publication can enable subsequent serving.
Existing `provider proxy add` retains its supervisor-installation meaning;
these setup commands never invoke it automatically.
