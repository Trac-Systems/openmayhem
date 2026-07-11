# Contributing

## Error Containment

Serving code assigns every failure an explicit blast radius:

- A request, session, peer message, or connection failure is logged and cleaned up at that boundary. It must not escape a long-running loop.
- A component failure is surfaced and restarted with bounded backoff. Restart loops must remain visible in health and status output.
- A process exits only for invalid startup configuration, a failed network/security gate, or an unrecoverable top-level failure.

Rust dev and release builds use `panic = "unwind"`. Panics are caught only at explicit task or component boundaries; normal request errors use `Result`. A caught panic is reported as a component fault and must not be silently discarded. JavaScript event callbacks catch remote-input failures at the message or connection boundary. An otherwise unhandled exception or rejection is fatal and is left to the process supervisor after it has been logged.

Review every new long-running loop and callback for `?`, `return Err`, `unwrap`, `expect`, `panic`, rejected promises, and dropped task results. Tests must inject the relevant failure and prove that unrelated work continues.
