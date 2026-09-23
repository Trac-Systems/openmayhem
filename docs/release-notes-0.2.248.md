# OpenMayhem Core 0.2.248

Concurrent embedding runtimes now expose and use their signed shared-scheduler
capacity. The scheduler is bounded by the catalog limit and the provider's
configured session limit without being reduced by full-model replica capacity.
Runtimes without a concurrent backend remain exclusive.

This release also includes the durable terminal-result recovery and calibrated
large-batch embedding limits introduced in 0.2.246.

Contract version 26 and its authenticated history are unchanged.
