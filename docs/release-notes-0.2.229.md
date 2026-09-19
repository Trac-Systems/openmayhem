# OpenMayhem 0.2.229

- Posts and persists matched TNK deposits before shutting down the temporary ledger reader.
- Bounds ledger-reader shutdown so a transport close failure cannot suppress confirmed TNK credit.
- Includes the retail TNK/TAP worker reliability fixes from 0.2.228.

The Intercom contract remains at version 25; this release requires no ledger migration.
