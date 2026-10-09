# Operator-approved real-backend acceptance

The ignored `approved_live_backend_protocol_and_recovery` test sends exactly one
inference invocation through the real connector, isolated decoder, endpoint
validation, observable metering and retained-result journal. It then verifies
that the original invocation cannot be dispatched again. No production proxy
registration or settlement is created. Its reservation/rail data is explicitly
a disposable local fixture, so success is not real-payment acceptance.

Run only after the operator approves use of an already running upstream. Check
its current activity first and use bounded, sequential requests. Do not stop or
reconfigure the native service, infer extra global capacity from an idle sample,
or fault-inject into a production backend.

Set `MAYHEM_PROXY_LIVE_APPROVED=1`, `MAYHEM_PROXY_LIVE_INPUT` to an owner-only JSON
file, and optionally `MAYHEM_PROXY_LIVE_REPORT` to a new private report filename.
The input contains `connection_file` (absolute path to a protected ordinary
ConnectionConfig), `upstream_model`, `endpoint`, `request`, and
`timeout_seconds` (1–300). The test replaces only the public request model with
`public-model`; the adapter resolves it to the explicitly configured upstream.
Do not put credentials in this input: use ConnectionConfig secret references.

```
cargo test -p mayhem-proxy --test execution \
  live_backend::approved_live_backend_protocol_and_recovery \
  -- --ignored --exact --nocapture
```

The input/request is limited to64KiB and an LLM call must specify an explicit
output limit at most1024 tokens. These are allowances for this opt-in test,
not product limits. The test deadline does not permit an automatic retry after
unknown execution. Cases must be selected deliberately (JSON, streaming,
structured output, tools, or an available decisions interface); one passing
case is not a capability claim for all of them. A custom upstream protocol
requires its own signed adapter configuration and corresponding acceptance.
Complete-call acceptance requires a complete validated result. A required-tool
case additionally requires a completed tool call; a protocol-valid response
that exhausted its output budget is retained as partial evidence, not a pass.

Reports omit prompts, answers, credentials and upstream addresses. Stream
first-output time excludes role-only/bookkeeping events. The generation rate
uses the backend's reported token count over the observed generation interval;
it is approximate and is not independently attested native-token performance.
Proxy billing units remain independently observed and may differ. Compare the
same input/backend/settings with a direct baseline, record cache/load state,
and do not call a single warm comparison a general overhead guarantee.

Keep failing evidence as well as the targeted repair result. Capture a bounded
raw response privately only when diagnosing that specific synthetic test.
Never enable ongoing customer-prompt logging for this test.
