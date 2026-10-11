# OpenMayhem 0.2.273

Proxied sellers can configure several models in one Core home with
`mayhem provider proxy setup init --name <name>`. Each setup has its own upstream
model, market, offers and managed controller. Names contain 1 to 32 lowercase
letters, digits or hyphens and default to `default`.

New bundles live in `<home>/proxy-setup/<name>`. An existing bundle containing
`<home>/proxy-setup/wizard.json` remains the setup named `default` at that path.
Core preserves its files, cumulative budgets and installed controller identity.

`mayhem provider proxy setup list` returns a page of local setup names, models,
markets and retained publication and Run states. The command reads local
snapshots without contacting services or creating locks. The wizard,
payment-registration, draft inspection and recovery commands accept `--name`
and `--home`. The local dashboard uses the same selection with
`mayhem use --proxy-setup --name <name>` or
`mayhem use --proxy-setup-config --name <name>`.

The wizard reads canonical admission by provider identity before requesting
another invoice. An already admitted provider needs no second invoice or
admission permit to publish another market. The canonical publication checks
still verify the provider's current sequence and registration state.

Named setups have separate controller names, runtime and worker directories,
locks, journals, capacity groups, routes and cumulative probe allowances.
They share the existing wallet, Core peer, bridge and supervisor. Managed
controllers do not require another HTTP port or OS service.

This release changes the client. The ledger contract remains at version 30.
