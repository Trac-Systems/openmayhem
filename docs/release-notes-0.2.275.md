# OpenMayhem 0.2.275

Proxy controllers can exhaust their bounded bridge queue after many completed
control sessions. The opening listener previously consumed frames while retaining
unrelated session-close notifications indefinitely. It now consumes those
notifications too. Established sessions still receive their own close events;
queue limits and authenticated opening validation remain unchanged.

A discovery message can also arrive between bridge authentication and the
session-only subscription. That already-queued message previously caused the
paid-session handoff to reject its own connection before execution. The handoff
now discards only pre-existing discovery events. Queued session frames and close
notifications remain intact; wrong-session messages, invalid replies, subsequent
unexpected traffic and queue limits retain their existing checks.

A proxy reservation can be accepted before its canonical financial observation
is available. Previously, a temporary read failure could leave a never-executed
purchase pending indefinitely. Financial observation now retries transport
failures, HTTP 429 and HTTP 5xx at most three times within a total 15-second
deadline, using a fresh challenge for each read. Successful responses still pass
all existing signature, identity, freshness and financial checks. Invalid
observations are not accepted, and these retries publish no transactions.

Proxy financial publication now checks the application acknowledgment inside an
HTTP-success response. Explicit temporary accounting dependency or journal
capacity conflicts retry the **same immutable signed operation**, with at most
ten attempts and a total ten-second deadline. Permanent rejections, invalid
responses and ambiguous transport failures are not blindly republished. The
existing canonical journal retains its accounting fences and deduplication.
Reservation admission can wait for delayed canonical visibility with at most
twelve fresh reads inside fifteen seconds; ordinary observation keeps its
three-read budget. Neither publication acknowledgment nor a timeout authorizes
inference, settlement, a replacement hold or release of customer funds.

Recovery still never sends another Execute. If the provider reports that the
original attempt is Prepared, recovery requests cancellation and waits for a
signed, verified zero-charge closure. It also consumes retained results or
closure evidence following CancelRequested. A state report, disconnection or
timeout alone cannot release money or prove that execution stopped. Running and
unknown outcomes remain subject to the existing recovery rules.

If Execute never ran, the provider may not yet have retained the canonical
funding snapshot. After its durable pre-dispatch cancellation fence, it can
recover that exact still-open reservation for closure, including after execution
permission expires. This path cannot create execution permission; it retains the
existing terms, signatures, identity checks, quotas and dispatch fence.

## Operator update

Proxy LLM health no longer treats a buffered completion's full generation time
as first-token latency. Longer valid answers cannot reduce concurrency simply
because earlier answers were shorter. Streaming latency and independently
measured native throughput retain their checks. Expired workload-specific
performance evidence can recover through a fresh bounded probe, reopening one
slot before capacity ramps; timers alone never restore availability.

Optional `MAYHEM_PROXY_DIAGNOSTICS=1` records bounded session failure categories
at most once per 30 seconds. It omits raw errors, prompts, URLs and credentials.
Disable it by removing the variable and restarting the affected controller.

Update affected proxy provider controllers and proxy buyer gateways. Install the
matching proxy worker with each buyer/controller that uses one; its IPC handshake
requires the same release version. Preserve all configuration, wallets,
negotiation journals, reservation records and peer stores. A restart must retain
the corrected executable and worker paths.

The ledger remains contract 30 with the same code digest. This release requires
no ledger migration, recalibration, model-runtime restart or native provider-fleet
rollout. Native inference, pricing and payment-rail rules are unchanged.

Regression coverage includes close-event turnover beyond queue capacity,
independent delivery of close events, transient and permanent financial read
failures, malformed financial responses, exact-once generation and verified
nonexecution closure across FIAT, TNK and TAP, including streaming.
