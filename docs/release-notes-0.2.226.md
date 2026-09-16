# OpenMayhem 0.2.226

- Keeps the TNK ledger reader synchronized between purchases so confirmation does not replay an idle reader’s entire backlog.
- Runs TAP and TNK discovery and health reporting independently from long-running payment settlement work.
- Bounds TNK confirmation scans from the caught-up ledger frontier.

The Intercom contract remains at version 25; this release requires no ledger migration.
