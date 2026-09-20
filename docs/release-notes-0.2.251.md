# OpenMayhem Core 0.2.251

Provider admission now keeps canonical spend-reservation confirmation separate
from route-discovery timing. Pending reservations retain their exact signed
identity and durable recovery evidence, preventing a delayed acknowledgement
from creating a conflicting retry or an incorrect capacity failure.

Intercom feature relays preserve accepted-but-pending status, and reservation
recovery treats locally absent canonical state as pending propagation rather
than a binding mismatch.

Contract version 26 and its authenticated history are unchanged.
