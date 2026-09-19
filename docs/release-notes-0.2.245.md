# OpenMayhem Core 0.2.245

Completed durable inference jobs now remain successful when a concurrent
receipt-settlement recovery finishes after the original handoff reports a
local error. The terminal job and its exact signed receipt take precedence,
so streaming clients receive the completed result instead of a contradictory
provider failure.

Contract version 26 and its authenticated history are unchanged.
