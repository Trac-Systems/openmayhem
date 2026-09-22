# OpenMayhem Core 0.2.222

- Add a canonical endpoint-contract fingerprint alongside the existing legacy fingerprint.
- Prefer the canonical fingerprint on updated providers while retaining legacy mixed-version compatibility.
- Keep semantically identical workflow contracts valid across object reordering, JavaScript relays, and architecture-specific builds.

This release does not change the Intercom contract.
