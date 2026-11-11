# 0.2.201

Runs the persistent managed model service with the same numeric user and group as
its Core-owned runtime directory. Signed source, prepared model files, caches, and
the long-running model process therefore share one ownership boundary from setup
through serving and cleanup.

The service remains constrained by its signed container image, resource limits,
network and IPC profile, seccomp policy, launch arguments, and artifact proofs.
Contract version remains 25 and this release requires no pricing migration.
