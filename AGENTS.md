# OpenMayhem rollout rule

Before every Core acceptance test or rollout, enumerate every active Core-managed process and every configured start path on each participating host. Include system and user services, timers, scheduled tasks, gateways, helpers, payment and payout workers, provider workers, model sidecars, wrappers, and child processes. Read the effective service configuration, environment-file paths, executable paths, working directories, and running process tree. A release directory on disk is not proof that a process uses it.

After cutover, repeat that inventory and require every active component and every timer or task that can start later to point to the exact target source release. Record version, commit, contract digest, and canonical applied-view proof for each participating peer. Resolve every stale component before declaring the rollout complete. A matching contract digest alone does not make an old Core worker current. Never delete or rebuild the canonical indexer store to align peers.

Follow the private `docs/knowledge/operations/fleet-rollout-and-sparse-stores.md` playbook for the full store and cutover procedure. Do not publish private fleet details or credentials.
