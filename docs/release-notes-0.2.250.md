# OpenMayhem Core 0.2.250

Gateway modality admission now avoids double-counting provider heartbeat load
and the gateway's overlapping local reservations. Provider-side atomic
capacity refusals remain the final authority and are treated as clean capacity
only when no execution or billing evidence exists, so healthy routes are not
cooled for ordinary contention while ambiguous work remains fail-closed.

Contract version 26 and its authenticated history are unchanged.
