# OpenMayhem Core 0.2.247

Concurrent embedding runtimes now expose their measured session capacity to
provider admission. Independent embedding requests can use available runtime
slots concurrently, while runtimes without concurrent execution remain
exclusive.

This release also includes the durable terminal-result recovery and calibrated
large-batch embedding limits introduced in 0.2.246.

Contract version 26 and its authenticated history are unchanged.
