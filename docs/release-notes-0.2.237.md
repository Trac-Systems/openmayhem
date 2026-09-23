# OpenMayhem Core 0.2.237

Structured JSON output now checks requested schemas before dispatch. Constraints
that a generation grammar cannot enforce, including `uniqueItems`, remain in the
request contract and are validated against the completed output. Invalid or
unsupported schemas return a request error without cooling a healthy provider.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged.
