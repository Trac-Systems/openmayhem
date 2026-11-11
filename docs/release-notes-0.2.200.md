# 0.2.200

Fixes ownership of files created by managed-runtime preparation containers.
Offline wheel installation, model preparation, and verification now run with
the managed runtime directory's numeric user and group, so their outputs remain
readable and removable by the provider service after each container exits.

Preparation remains isolated from the network and keeps its signed command and
artifact checks. The persistent model service is started separately after those
checks pass. Contract version remains 25 and this release requires no pricing
migration.
